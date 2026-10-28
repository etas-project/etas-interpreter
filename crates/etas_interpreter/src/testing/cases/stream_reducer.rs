use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

const IMPORTS: &str = r#"
import std.stream.{ByteStream, ByteLimit, StreamError, StreamRead, Timeout, read_until_limit};
import std.effects.Stream;
import std.option.{Some, None};
import std.runtime.checkpoint;
import std.codec.text.utf8_encode;
type Buffer = string;
impl Buffer ~ ByteStream;
"#;

async fn execute(checked: &CheckedProject, host: &FakeHost) -> api::RunResult {
    Interpreter
        .run_checked(
            checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            host,
            RunOptions::default(),
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn stream_reducer_dispatches_each_read_and_probes_eof_at_the_exact_limit() {
    let checked = checked_project(
        r#"
module app.main;
import std.net.tcp.{Host, Port, TcpOptions, connect};
import std.stream.{ByteLimit, Timeout, read_until_limit};
import std.option.None;
flow main() -> bytes {
    let stream = connect(Host { host = "example.test" }, Port { port = 80 }, TcpOptions {});
    return read_until_limit(stream, ByteLimit { bytes = 4 }, None<Timeout>());
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Tcp,
        HostRequirementKind::Stream,
    ]));
    host.seed_tcp_connect_stream("stream", "example.test", 80);
    host.seed_stream_read_data(b"ab");
    host.seed_stream_read_data(b"cd");
    host.seed_stream_eof();
    let result = execute(&checked, &host).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&value::InterpValue::Bytes(b"abcd".to_vec().into()))
    );
    let requests = host.stream_requests();
    assert_eq!(requests.len(), 3);
    for (request, expected) in requests.iter().zip([5, 3, 1]) {
        assert!(
            matches!(request.operation, etas_host::StreamOperation::Read { max_bytes, .. } if max_bytes == expected),
            "{request:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn stream_reducer_obeys_each_source_handler_resume_without_a_host_handle() {
    let checked = checked_project(&format!(
        r#"
module app.main; {IMPORTS}
flow main() -> bytes ![Error<StreamError>] {{
    var reads = 0;
    return handle {{ read_until_limit(Buffer("source"), ByteLimit {{ bytes = 4 }}, None<Timeout>()) }} with {{
        Stream.read<Buffer>(_) => {{
            reads = reads + 1;
            let response = match reads {{ 1 => StreamRead.Data(utf8_encode("ab")),
                2 => StreamRead.Data(utf8_encode("cd")), _ => StreamRead.Eof, }};
            resume response;
        }}
    }};
}}
"#
    ));
    let host = FakeHost::new(HostServiceAvailability::default());
    let result = execute(&checked, &host).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&value::InterpValue::Bytes(b"abcd".to_vec().into()))
    );
    assert!(host.stream_requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn stream_reducer_reports_its_own_overflow_after_the_read_action_is_handled() {
    let checked = checked_project(&format!(
        r#"
module app.main; {IMPORTS}
flow main() -> i32 ![] {{
    return handle {{
        let unused = read_until_limit(Buffer("source"), ByteLimit {{ bytes = 4 }}, None<Timeout>()) with {{
            Stream.read<Buffer>(_) => {{ resume StreamRead.Data(utf8_encode("abc")); }}
        }};
        0
    }} with {{
        Error<StreamError>.raise(StreamError.LimitExceeded) => {{ finish 11; }}
        Error<StreamError>.raise(_) => {{ finish 12; }}
    }};
}}
"#
    ));
    let host = FakeHost::new(HostServiceAvailability::default());
    let result = execute(&checked, &host).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(11)));
    assert!(host.stream_requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn stream_reducer_finish_does_not_run_the_remainder_or_default_boundary() {
    let checked = checked_project(&format!(
        r#"
module app.main; {IMPORTS}
flow main() -> bytes ![Error<StreamError>] {{
    return handle {{
        let unused = read_until_limit(Buffer("source"), ByteLimit {{ bytes = 4 }}, None<Timeout>());
        utf8_encode("incorrect")
    }} with {{ Stream.read<Buffer>(_) => {{ finish utf8_encode("finished"); }} }};
}}
"#
    ));
    let host = FakeHost::new(HostServiceAvailability::default());
    let result = execute(&checked, &host).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&value::InterpValue::Bytes(b"finished".to_vec().into()))
    );
    assert!(host.stream_requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn stream_reducer_zero_deadline_raises_timed_out_without_requesting_a_read() {
    let checked = checked_project(&format!(
        r#"
module app.main; {IMPORTS}
flow main() -> i32 ![] {{
    return handle {{
        let unused = read_until_limit(Buffer("source"), ByteLimit {{ bytes = 4 }}, Some(Timeout {{ ms = 0 }})) with {{
            Stream.read<Buffer>(_) => {{ finish utf8_encode("should not run"); }}
        }};
        0
    }} with {{
        Error<StreamError>.raise(StreamError.TimedOut) => {{ finish 13; }}
        Error<StreamError>.raise(_) => {{ finish 14; }}
    }};
}}
"#
    ));
    let host = FakeHost::new(HostServiceAvailability::default());
    let result = execute(&checked, &host).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(13)));
    assert!(host.stream_requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn stream_reducer_checkpoint_restores_the_buffer_and_pending_read_continuation() {
    let checked = checked_project(&format!(
        r#"
module app.main; {IMPORTS}
flow main() -> bytes ![Error<StreamError>] {{
    var reads = 0;
    return handle {{ read_until_limit(Buffer("source"), ByteLimit {{ bytes = 4 }}, None<Timeout>()) }} with {{
        Stream.read<Buffer>(_) => {{
            reads = reads + 1;
            let response = if reads == 1 {{ StreamRead.Data(utf8_encode("ab")) }} else {{
                checkpoint("buffered stream");
                StreamRead.Eof
            }};
            resume response;
        }}
    }};
}}
"#
    ));
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let result = execute(&checked, &host).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&value::InterpValue::Bytes(b"ab".to_vec().into()))
    );
    assert_eq!(result.checkpoints.len(), 1);
    let wire = checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    let saved = checkpoint_from_json(&wire, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &saved, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), result.value());
    assert!(host.stream_requests().is_empty());
    for change in ["limit", "body", "call"] {
        let mut invalid = wire.clone();
        let reducer = find_reducer(&mut invalid).expect("reducer continuation is persisted");
        match change {
            "limit" => reducer["limit"] = serde_json::json!(100),
            "body" => reducer["body"] = serde_json::json!({"kind":"string", "value":"not bytes"}),
            "call" => {
                reducer["call"] =
                    serde_json::json!({"kind":"flow", "item":checked.entry.unwrap().0})
            }
            _ => unreachable!(),
        }
        assert!(
            checkpoint_from_json(&invalid, &checked).is_err(),
            "accepted corrupt reducer {change}"
        );
    }
}

fn find_reducer(value: &mut serde_json::Value) -> Option<&mut serde_json::Value> {
    if value.get("kind").and_then(serde_json::Value::as_str) == Some("stream_read_reduction") {
        return Some(value);
    }
    match value {
        serde_json::Value::Object(fields) => fields.values_mut().find_map(find_reducer),
        serde_json::Value::Array(values) => values.iter_mut().find_map(find_reducer),
        _ => None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn stream_reducer_empty_data_is_not_eof_and_still_consumes_execution_fuel() {
    let checked = checked_project(&format!(
        r#"
module app.main; {IMPORTS}
flow main() -> bytes ![Error<StreamError>] {{
    return handle {{ read_until_limit(Buffer("source"), ByteLimit {{ bytes = 4 }}, None<Timeout>()) }} with {{
        Stream.read<Buffer>(_) => {{ resume StreamRead.Data(utf8_encode("")); }}
    }};
}}
"#
    ));
    let host = FakeHost::new(HostServiceAvailability::default());
    let mut options = RunOptions::default();
    options.execution_limits.max_steps = std::num::NonZeroU64::new(256);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(result.value().is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("maximum interpreter execution steps (256)")),
        "{:?}",
        result.diagnostics
    );
    assert!(host.stream_requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn stream_reducer_zero_byte_limit_requires_eof_instead_of_silently_truncating() {
    for (response, expected) in [
        ("StreamRead.Eof", 0),
        ("StreamRead.Data(utf8_encode(\"x\"))", 1),
    ] {
        let checked = checked_project(&format!(
            r#"
module app.main; {IMPORTS}
flow main() -> i32 ![] {{
    return handle {{
        let unused = read_until_limit(Buffer("source"), ByteLimit {{ bytes = 0 }}, None<Timeout>()) with {{
            Stream.read<Buffer>(_) => {{ resume {response}; }}
        }};
        0
    }} with {{ Error<StreamError>.raise(_) => {{ finish 1; }} }};
}}
"#
        ));
        let host = FakeHost::new(HostServiceAvailability::default());
        let result = execute(&checked, &host).await;
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&value::InterpValue::i32(expected)));
        assert!(host.stream_requests().is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn stream_reducer_checkpoint_resume_does_not_restart_its_timeout() {
    let checked = checked_project(&format!(
        r#"
module app.main; {IMPORTS}
flow main() -> i32 ![] {{
    var reads = 0;
    return handle {{
        let unused = read_until_limit(Buffer("source"), ByteLimit {{ bytes = 4 }}, Some(Timeout {{ ms = 2000 }})) with {{
            Stream.read<Buffer>(_) => {{
                reads = reads + 1;
                let response = if reads == 1 {{ StreamRead.Data(utf8_encode("ab")) }} else {{
                    checkpoint("stream deadline");
                    StreamRead.Eof
                }};
                resume response;
            }}
        }};
        0
    }} with {{ Error<StreamError>.raise(StreamError.TimedOut) => {{ finish 17; }} Error<StreamError>.raise(_) => {{ finish 18; }} }};
}}
"#
    ));
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let result = execute(&checked, &host).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(0)));
    assert_eq!(result.checkpoints.len(), 1);
    let wire = checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    let saved = checkpoint_from_json(&wire, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &saved, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&value::InterpValue::i32(17)));
}

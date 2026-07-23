use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_local_flow_and_returns_value() {
    let checked = checked_project(
        r#"
module app.main;

flow helper(value: string) -> string {
  return value;
}

flow main() -> string {
  let result = helper("ok");
  return result;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("ok".to_owned()))
    );
    assert!(!result.events.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_if_expression_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> string {
  let result = if !false && 1 < 2 { "yes" } else { "no" };
  return result;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("yes".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_if_statement_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> string {
  if false {
    return "bad";
  }

  if true {
    return "ok";
  }

  return "fallback";
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("ok".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_match_expression_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> string {
  let value = match (true, "ok") {
    (flag, text) => text
  };
  return value;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("ok".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_literal_match_patterns_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow classify(value: i32) -> string {
  return match value {
    0 => "zero",
    1 => "one",
    _ => "many"
  };
}

flow main() -> string {
  return classify(1);
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("one".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_std_bytes_and_result_helpers_without_host() {
    let checked = checked_project(
        r#"
module app.main;
import std.bytes.len;
import std.codec.text.utf8_encode;
import std.result.{is_err, is_ok};

flow main() -> bool {
  let encoded = utf8_encode("abc");
  let ok: Result<string, string> = Ok("ok");
  let err: Result<string, string> = Err("bad");
  return len(encoded) == 3 && is_ok(ok) && is_err(err);
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_distinct_option_and_result_unwrap_intrinsics() {
    let checked = checked_project(
        r#"
module app.main;
import std.option.unwrap as option_unwrap;
import std.result.unwrap as result_unwrap;

flow main() -> i32 {
  let some: Option<i32> = Some(7);
  let ok: Result<i32, string> = Ok(11);
  return option_unwrap(some) + result_unwrap(ok);
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(18)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_option_unwrap_none_from_source() {
    let checked = checked_project(
        r#"
module app.main;
import std.option.unwrap;

flow main() -> i32 {
  let none: Option<i32> = None();
  return unwrap(none);
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert_eq!(result.value, None);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unwrap encountered None")),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_result_unwrap_err_from_source() {
    let checked = checked_project(
        r#"
module app.main;
import std.result.unwrap;

flow main() -> i32 {
  let err: Result<i32, string> = Err("failed");
  return unwrap(err);
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert_eq!(result.value, None);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unwrap encountered Err")),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_matches_http_codec_result_variants_without_host() {
    let checked = checked_project(
        r#"
module app.main;
import std.codec.text.utf8_encode;
import std.http.codec.{MalformedMessage, decode_response};

flow main() -> string {
  return match decode_response(utf8_encode("not http")) {
    Ok(_) => "unexpected",
    Err(MalformedMessage) => "malformed",
    Err(_) => "other"
  };
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("malformed".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_decodes_http_chunked_response_body_without_host() {
    let checked = checked_project(
        r#"
module app.main;
import std.bytes.len;
import std.codec.text.utf8_encode;
import std.http.codec.decode_response;

flow main() -> bool {
  return match decode_response(utf8_encode("HTTP/1.1 200 OK\nTransfer-Encoding: chunked\n\n5\nhello\n6;ext=value\n world\n0\n\n")) {
    Ok(response) => len(response.body) == 11,
    Err(_) => false
  };
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_utf8_decode_with_malformed_input_value_without_host() {
    let checked = checked_project(
        r#"
module app.main;
import std.codec.text.{Strict, utf8_decode, utf8_encode};

flow main() -> string {
  return match utf8_decode(utf8_encode("hello"), Strict) {
    Ok(text) => text,
    Err(_) => "bad"
  };
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("hello".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_maps_stream_limit_overflow_to_limit_exceeded_variant() {
    let checked = checked_project(
        r#"
module app.main;
import std.net.tcp.{Host, NetworkError, Port, TcpOptions, connect};
import std.option.None;
import std.stream.{ByteLimit, LimitExceeded, StreamError, Timeout, read_until_limit};

flow main() -> string ![Error<NetworkError>] {
  return handle {
    let stream = connect(Host { host = "example.test" }, Port { port = 80 }, TcpOptions {});
    let _body = read_until_limit(stream, ByteLimit { bytes = 4 }, None<Timeout>());
    "unexpected"
  } with {
    Error<StreamError>.raise(LimitExceeded) => {
      finish "limit";
    }
    Error<StreamError>.raise(_) => {
      finish "other";
    }
    Error<NetworkError>.raise(_) => {
      finish "network";
    }
  };
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Tcp,
        HostRequirementKind::Stream,
    ]));
    host.seed_tcp_connect_stream("tcp-1", "example.test", 80);
    host.seed_stream_read_until_limit_error(
        HostErrorCode::BudgetExceeded,
        "stream read exceeded byte limit before EOF",
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("limit".to_owned()))
    );
    let tcp_requests = host.tcp_requests();
    assert_eq!(tcp_requests.len(), 1, "{tcp_requests:#?}");
    let requests = host.stream_requests();
    assert_eq!(requests.len(), 1, "{requests:#?}");
    match &requests[0].operation {
        etas_host::StreamOperation::ReadUntilLimit {
            stream,
            limit_bytes,
            ..
        } => {
            assert_eq!(stream.id, "tcp-1");
            assert_eq!(*limit_bytes, 4);
        }
        other => panic!("expected ReadUntilLimit stream operation, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_maps_tcp_host_failure_to_network_error() {
    let checked = checked_project(
        r#"
module app.main;
import std.net.tcp.{Host, NetworkError, Port, TcpOptions, connect};

flow main() -> string {
  return handle {
    let _stream = connect(Host { host = "example.test" }, Port { port = 80 }, TcpOptions {});
    "connected"
  } with {
    Error<NetworkError>.raise(_) => {
      finish "network";
    }
  };
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Tcp]));
    host.seed_tcp_connect_error(HostErrorCode::ProviderUnavailable, "connection refused");

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("network".to_owned()))
    );
    assert_eq!(host.tcp_requests().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_preserves_typed_network_error_identity_in_handler_binding() {
    let checked = checked_project(
        r#"
module app.main;
import std.net.tcp.{Host, NetworkError, Port, TcpOptions, connect};

flow main() -> NetworkError {
  return handle {
    let _stream = connect(Host { host = "example.test" }, Port { port = 80 }, TcpOptions {});
    abort("connection unexpectedly succeeded")
  } with {
    Error<NetworkError>.raise(error) => {
      finish error;
    }
  };
}
"#,
    );
    let expected_type = checked
        .entry
        .and_then(|item| checked.types.item_signatures.get(&item))
        .and_then(|signature| match signature {
            etas_types::ItemSignature::Flow(flow) => Some(flow.output),
            _ => None,
        })
        .expect("checked NetworkError output type");
    let host = FakeHost::new(availability(&[HostRequirementKind::Tcp]));
    host.seed_tcp_connect_error(HostErrorCode::ProviderUnavailable, "connection refused");

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(value::InterpValue::Nominal { ty, value }) = result.value else {
        panic!("expected typed network error value, got {:?}", result.value);
    };
    assert_eq!(ty, expected_type);
    let value::InterpValue::Record(fields) = value.as_ref() else {
        panic!("expected structured opaque host error payload, got {value:?}");
    };
    assert!(fields.borrow().iter().any(|(name, value)| {
        name == "code" && value == &value::InterpValue::String("ProviderUnavailable".to_owned())
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_maps_postfix_stream_limit_handler_after_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;
import std.bytes.len as bytes_len;
import std.codec.text.utf8_encode;
import std.net.tcp.{Host, NetworkError, Port, TcpOptions, connect};
import std.option.None;
import std.stream.{ByteLimit, LimitExceeded, StreamError, Timeout, read_until_limit};

flow main() -> string ![Error<NetworkError>] {
  let stream = connect(Host { host = "example.test" }, Port { port = 80 }, TcpOptions {});
  let body = read_until_limit(stream, ByteLimit { bytes = 4 }, None<Timeout>()) with {
    Error<StreamError>.raise(LimitExceeded) => {
      finish utf8_encode("limit");
    }
    Error<StreamError>.raise(_) => {
      finish utf8_encode("other");
    }
  };
  if bytes_len(body) == bytes_len(utf8_encode("limit")) {
    return "limit";
  }
  return "unexpected";
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Tcp,
        HostRequirementKind::Stream,
    ]));
    host.seed_tcp_connect_stream("tcp-1", "example.test", 80);
    host.seed_stream_read_until_limit_error(
        HostErrorCode::BudgetExceeded,
        "stream read exceeded byte limit before EOF",
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("limit".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_preserves_nested_stream_handler_inside_resumed_handler_value() {
    let checked = checked_project(
        r#"
module app.main;
import std.bytes.len as bytes_len;
import std.codec.text.utf8_encode;
import std.net.tcp.{Host, NetworkError, Port, TcpOptions, TcpStream, connect};
import std.option.None;
import std.stream.{ByteLimit, LimitExceeded, StreamError, Timeout, read_until_limit};

effect Gateway {
  action request(stream: TcpStream) -> bytes;
}

flow read_inner(stream: TcpStream) -> bytes ![] {
  return read_until_limit(stream, ByteLimit { bytes = 4 }, None<Timeout>()) with {
    Error<StreamError>.raise(LimitExceeded) => {
      finish utf8_encode("limit");
    }
    Error<StreamError>.raise(_) => {
      finish utf8_encode("other");
    }
  };
}

let GatewayDefault: ![Gateway => [] for bytes] = handler {
  Gateway.request(stream) => {
    resume read_inner(stream);
  }
};

flow main() -> string ![Error<NetworkError>] {
  let stream = connect(Host { host = "example.test" }, Port { port = 80 }, TcpOptions {});
  let body = perform Gateway.request(stream) with GatewayDefault;
  if bytes_len(body) == bytes_len(utf8_encode("limit")) {
    return "limit";
  }
  return "unexpected";
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Tcp,
        HostRequirementKind::Stream,
    ]));
    host.seed_tcp_connect_stream("tcp-1", "example.test", 80);
    host.seed_stream_read_until_limit_error(
        HostErrorCode::BudgetExceeded,
        "stream read exceeded byte limit before EOF",
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("limit".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_propagates_error_raised_from_finish_inside_resumed_handler_value() {
    let checked = checked_project(
        r#"
module app.main;
import std.net.tcp.{Host, NetworkError, Port, TcpOptions, TcpStream, connect};
import std.option.None;
import std.stream.{ByteLimit, LimitExceeded, StreamError, Timeout, read_until_limit};

type HttpError = { kind: string };

effect Gateway {
  action request(stream: TcpStream) -> bytes;
}

flow raise_http_error(error: HttpError) -> never ![Error<HttpError>] {
  return perform Error<HttpError>.raise(error);
}

flow read_inner(stream: TcpStream) -> bytes ![Error<HttpError>] {
  return read_until_limit(stream, ByteLimit { bytes = 4 }, None<Timeout>()) with {
    Error<StreamError>.raise(LimitExceeded) => {
      finish raise_http_error(HttpError { kind = "limit" });
    }
    Error<StreamError>.raise(_) => {
      finish raise_http_error(HttpError { kind = "other" });
    }
  };
}

let GatewayDefault: ![Gateway => Error<HttpError> for bytes] = handler {
  Gateway.request(stream) => {
    resume read_inner(stream);
  }
};

flow main() -> string ![Error<NetworkError>] {
  let stream = connect(Host { host = "example.test" }, Port { port = 80 }, TcpOptions {});
  return handle {
    let _body = perform Gateway.request(stream) with GatewayDefault;
    "unexpected"
  } with {
    Error<HttpError>.raise(err) => {
      finish err.kind;
    }
  };
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Tcp,
        HostRequirementKind::Stream,
    ]));
    host.seed_tcp_connect_stream("tcp-1", "example.test", 80);
    host.seed_stream_read_until_limit_error(
        HostErrorCode::BudgetExceeded,
        "stream read exceeded byte limit before EOF",
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("limit".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_match_statement_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> string {
  match (false, "bad") {
    (left, right) => {
      let seen = right;
    }
  }

  return "ok";
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("ok".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_var_assign_and_while_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> i32 {
  var i = 0;
  var sum = 0;

  while i < 4 limit Iterations(8) {
    sum = sum + i;
    i = i + 1;
  }

  return sum;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(6)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_break_and_continue_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> i32 {
  var i = 0;
  var seen = 0;

  while i < 5 limit Iterations(8) {
    i = i + 1;

    if i == 2 {
      continue;
    }

    if i == 4 {
      break;
    }

    seen = seen + i;
  }

  return seen;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(4)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_for_push_and_index_without_host() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> i32 {
  let values = [1, 2, 3];
  var out = [0];

  for value in values limit Iterations(8) {
    out = out.push(value + 10);
  }

  let idx = 2;
  return out[idx];
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(12)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_unhandled_index_error_instead_of_unimplemented_perform() {
    let checked = checked_project(
        r#"
module app.main;

flow main(args: Array<string>) -> string ![Error<IndexError>] {
  return args[0];
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::Array(
                value::ArrayValue::new(Vec::new()),
            )],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                != DiagnosticCode::Analysis(AnalysisDiagnosticCode::ExecutionNotImplemented)
                && diagnostic
                    .message
                    .contains("Error[std.runtime.error.IndexError]")
                && diagnostic.message.contains("array index 0")
        }),
        "{:?}",
        result.diagnostics
    );
    let index_error_diagnostics = result
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .message
                .contains("Error[std.runtime.error.IndexError]")
                && diagnostic.message.contains("array index 0")
        })
        .count();
    assert_eq!(
        index_error_diagnostics, 1,
        "unhandled IndexError should be reported once: {:?}",
        result.diagnostics
    );
    assert!(
        result.diagnostics.iter().all(|diagnostic| diagnostic.code
            != DiagnosticCode::Analysis(AnalysisDiagnosticCode::ExecutionNotImplemented)),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_unhandled_effect_action_instead_of_unimplemented_perform() {
    let checked = checked_project(
        r#"
module app.main;

effect Network {
  action search(q: string) -> string;
}

flow main() -> string ![Network.search] {
  return perform Network.search("docs");
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(availability(&[HostRequirementKind::Network])),
            RunOptions::default(),
        )
        .await;

    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            format!("{:?}", diagnostic.code).contains("UnhandledEffectAction")
                && diagnostic.message.contains("Network.search")
        }),
        "{:?}",
        result.diagnostics
    );
    assert!(
        result.diagnostics.iter().all(|diagnostic| diagnostic.code
            != DiagnosticCode::Analysis(AnalysisDiagnosticCode::ExecutionNotImplemented)),
        "{:?}",
        result.diagnostics
    );
}

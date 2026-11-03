use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};
use etas_host::{
    BoundedFileRead, FilesystemEntry, FilesystemOperation, WorkspacePathRef, WorkspaceRegionId,
};

const IMPORTS: &str = r#"
import std.fs.{Region, WorkspacePath, IOError, ReadLimitError, read_bytes_limited};
import std.effects.Fs;
import std.codec.text.utf8_encode;
import std.runtime.checkpoint;
type ProjectRoot;
impl ProjectRoot ~ Region;
"#;

#[test]
fn bounded_file_read_allocation_does_not_scale_with_file_length() {
    let workspace = etas_host::TestWorkspace::create("bounded-file-allocation").unwrap();
    let file = std::fs::File::create(workspace.path().join("large.bin")).unwrap();
    let root = workspace.root().unwrap();
    let sandbox = etas_host::FilesystemSandbox::new(etas_host::FilesystemPolicy::allow_workspace(
        root.clone(),
    ));
    for size in [4096, 1 << 30] {
        file.set_len(size).unwrap();
        let (result, allocations) = crate::testing::allocation::measure(|| {
            sandbox.read_file_limited(&root, std::path::Path::new("large.bin"), 2048)
        });
        assert_eq!(
            result.unwrap(),
            BoundedFileRead::LimitExceeded { max_bytes: 2048 }
        );
        assert!(
            allocations.bytes < 64 * 1024,
            "file size {size} allocated {} bytes",
            allocations.bytes
        );
    }
}

fn path_value() -> value::InterpValue {
    value::InterpValue::WorkspacePath(
        WorkspacePathRef::new(
            WorkspaceRegionId::new("app.main.ProjectRoot").unwrap(),
            "input.txt",
        )
        .unwrap(),
    )
}

async fn execute(
    checked: &CheckedProject,
    host: &FakeHost,
    mut args: Vec<value::InterpValue>,
) -> api::RunResult {
    args.insert(0, path_value());
    Interpreter
        .run_checked(
            checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            args,
            host,
            RunOptions::default(),
        )
        .await
        .unwrap()
}

fn program(body: &str) -> CheckedProject {
    checked_project(&format!("module app.main; {IMPORTS} {body}"))
}

#[tokio::test(flavor = "current_thread")]
async fn bounded_file_read_preserves_zero_and_exact_limit_and_requests_only_fs_read() {
    for (limit, bytes) in [(0, b"".as_slice()), (4, b"abcd".as_slice())] {
        let checked = program(&format!(
            r#"
flow main(path: WorkspacePath<ProjectRoot>) -> bytes ![Error<IOError>, Error<ReadLimitError>] {{
    return read_bytes_limited(path, {limit});
}}
"#
        ));
        let host = FakeHost::new(availability(&[HostRequirementKind::FileIO]));
        host.seed_filesystem_response(Ok(FilesystemEntry::LimitedRead(BoundedFileRead::Complete(
            bytes.to_vec(),
        ))));
        let result = execute(&checked, &host, vec![]).await;
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(
            result.value(),
            Some(&value::InterpValue::Bytes(bytes.to_vec().into()))
        );
        let requests = host.filesystem_requests();
        assert_eq!(requests.len(), 1);
        assert!(
            matches!(requests[0].operation, FilesystemOperation::ReadLimited { max_bytes, .. } if max_bytes == limit)
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn bounded_file_read_host_overflow_raises_the_checked_limit_error() {
    let checked = program(
        r#"
flow main(path: WorkspacePath<ProjectRoot>) -> usize ![Error<IOError>] {
    return handle { let unused = read_bytes_limited(path, 2048); 0 } with {
        Error<ReadLimitError>.raise(ReadLimitError.LimitExceeded(maximum)) => { finish maximum; }
    };
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::FileIO]));
    host.seed_filesystem_response(Ok(FilesystemEntry::LimitedRead(
        BoundedFileRead::LimitExceeded { max_bytes: 2048 },
    )));
    let result = execute(&checked, &host, vec![]).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::usize(2048)));
    assert_eq!(host.filesystem_requests().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn bounded_file_read_limit_survives_source_handler_interception() {
    for (bytes, expected) in [("abc", 1), ("abcd", 9)] {
        let checked = program(&format!(
            r#"
flow main(path: WorkspacePath<ProjectRoot>) -> i32 ![] {{
    return handle {{
        let contents = read_bytes_limited(path, 3) with {{
            Fs.read<ProjectRoot>(_) => {{ resume utf8_encode("{bytes}"); }}
        }};
        1
    }} with {{
        Error<ReadLimitError>.raise(ReadLimitError.LimitExceeded(_)) => {{ finish 9; }}
    }};
}}
"#
        ));
        let host = FakeHost::new(HostServiceAvailability::default());
        let result = execute(&checked, &host, vec![]).await;
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&value::InterpValue::i32(expected)));
        assert!(host.filesystem_requests().is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn bounded_file_read_io_failure_is_an_error_effect_not_a_partial_value() {
    let checked = program(
        r#"
flow main(path: WorkspacePath<ProjectRoot>) -> i32 ![Error<ReadLimitError>] {
    return handle { let unused = read_bytes_limited(path, 2048); 0 } with {
        Error<IOError>.raise(_) => { finish 7; }
    };
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::FileIO]));
    host.seed_filesystem_response(Err(etas_host::HostError::new(
        HostErrorCode::IoFailure,
        "test read failed",
    )));
    let result = execute(&checked, &host, vec![]).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(7)));
}

#[tokio::test(flavor = "current_thread")]
async fn bounded_file_read_rejects_malformed_host_response_instead_of_truncating() {
    for entry in [
        FilesystemEntry::Bytes(vec![1, 2]),
        FilesystemEntry::LimitedRead(BoundedFileRead::Complete(vec![1; 4])),
        FilesystemEntry::LimitedRead(BoundedFileRead::LimitExceeded { max_bytes: 99 }),
    ] {
        let checked = program(
            r#"
flow main(path: WorkspacePath<ProjectRoot>) -> bytes ![Error<IOError>, Error<ReadLimitError>] {
    return read_bytes_limited(path, 3);
}
"#,
        );
        let host = FakeHost::new(availability(&[HostRequirementKind::FileIO]));
        host.seed_filesystem_response(Ok(entry));
        let result = execute(&checked, &host, vec![]).await;
        assert!(
            !result.diagnostics.is_empty(),
            "malformed host response was accepted"
        );
        assert!(result.value().is_none());
        assert_eq!(host.filesystem_requests().len(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn bounded_file_read_checkpoint_preserves_pending_limit_and_checked_action() {
    let checked = program(
        r#"
flow limited<R ~ Region>(path: WorkspacePath<R>) -> bytes ![Fs.read<R>, Error<IOError>, Error<ReadLimitError>] {
    return read_bytes_limited(path, 3);
}
flow main(path: WorkspacePath<ProjectRoot>) -> bytes ![Error<ReadLimitError>] {
    return limited(path) with {
        Fs.read<ProjectRoot>(_) => {
            checkpoint("bounded-read-handler");
            resume utf8_encode("abc");
        }
    };
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let first = execute(&checked, &host, vec![]).await;
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.checkpoints.len(), 1);
    let artifact = checkpoint_artifact_json(&[], "main", &first.checkpoints[0]).unwrap();
    let saved = checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &saved, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), first.value());
    assert!(host.filesystem_requests().is_empty());
    for change in ["limit", "call", "args"] {
        let mut invalid = artifact.clone();
        let reducer = find_reducer(&mut invalid).expect("bounded read continuation persisted");
        match change {
            "limit" => reducer["limit"] = serde_json::json!(999),
            "call" => {
                reducer["call"] =
                    serde_json::json!({"kind":"flow", "item":checked.entry.unwrap().0})
            }
            "args" => reducer["args"] = serde_json::json!([]),
            _ => unreachable!(),
        }
        assert!(
            checkpoint_from_json(&invalid, &checked).is_err(),
            "accepted corrupt {change}"
        );
    }
}

fn find_reducer(value: &mut serde_json::Value) -> Option<&mut serde_json::Value> {
    if value.get("kind").and_then(serde_json::Value::as_str) == Some("file_read_reduction") {
        return Some(value);
    }
    match value {
        serde_json::Value::Object(fields) => fields.values_mut().find_map(find_reducer),
        serde_json::Value::Array(values) => values.iter_mut().find_map(find_reducer),
        _ => None,
    }
}

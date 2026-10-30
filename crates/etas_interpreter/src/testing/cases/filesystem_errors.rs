use super::super::*;
use etas_host::{
    FilesystemEntry, FilesystemOperation, HostError, WorkspacePathRef, WorkspaceRegionId,
};

fn path_value() -> WorkspacePathRef {
    WorkspacePathRef::new(
        WorkspaceRegionId::new("app.main.ProjectRoot").unwrap(),
        "missing.txt",
    )
    .unwrap()
}

fn program(call: &str) -> CheckedProject {
    checked_project(&format!(
        r#"
module app.main;
import std.fs.{{Region, WorkspacePath, IOError as FileError, ReadLimitError,
    stat, read_bytes, read_bytes_limited, write_bytes, list, atomic_replace}};
import std.codec.text.utf8_encode;
type ProjectRoot;
impl ProjectRoot ~ Region;
flow main(path: WorkspacePath<ProjectRoot>) -> i32 {{
    return handle {{ let unexpected = {call}; 1 }} with {{
        Error<FileError>.raise(_) => {{ finish 7; }}
        Error<ReadLimitError>.raise(_) => {{ finish 8; }}
    }};
}}
"#
    ))
}

async fn execute(checked: &CheckedProject, host: &FakeHost) -> api::RunResult {
    Interpreter
        .run_checked(
            checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![value::InterpValue::WorkspacePath(path_value())],
            host,
            RunOptions::default(),
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn filesystem_io_failures_raise_canonical_error_through_an_import_alias() {
    let path = path_value();
    for (call, expected) in [
        (
            "stat(path)",
            FilesystemOperation::Stat { path: path.clone() },
        ),
        (
            "read_bytes(path)",
            FilesystemOperation::Read { path: path.clone() },
        ),
        (
            "read_bytes_limited(path, 8)",
            FilesystemOperation::ReadLimited {
                path: path.clone(),
                max_bytes: 8,
            },
        ),
        (
            "write_bytes(path, utf8_encode(\"data\"))",
            FilesystemOperation::Write {
                path: path.clone(),
                contents: b"data".to_vec(),
                create_dirs: false,
            },
        ),
        (
            "list(path)",
            FilesystemOperation::ReadDir { path: path.clone() },
        ),
        (
            "atomic_replace(path, utf8_encode(\"data\"))",
            FilesystemOperation::AtomicReplace {
                path,
                contents: b"data".to_vec(),
            },
        ),
    ] {
        let checked = program(call);
        let host = FakeHost::new(availability(&[HostRequirementKind::FileIO]));
        host.seed_filesystem_response(Err(HostError::new(
            HostErrorCode::IoFailure,
            "resource I/O failed",
        )));
        let result = execute(&checked, &host).await;
        assert!(
            result.diagnostics.is_empty(),
            "{call}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.value(), Some(&value::InterpValue::i32(7)), "{call}");
        let requests = host.filesystem_requests();
        assert_eq!(requests.len(), 1, "{call}");
        assert_eq!(requests[0].operation, expected, "{call}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn filesystem_control_authority_and_protocol_failures_are_not_language_io_errors() {
    for call in ["stat(path)", "read_bytes_limited(path, 8)"] {
        let checked = program(call);
        for code in [
            HostErrorCode::AuthorityDenied,
            HostErrorCode::BudgetExceeded,
            HostErrorCode::TimedOut,
            HostErrorCode::Cancelled,
            HostErrorCode::Interrupted,
            HostErrorCode::InvalidRequest,
            HostErrorCode::InvalidResponse,
            HostErrorCode::SchemaMismatch,
            HostErrorCode::ProviderUnavailable,
        ] {
            let host = FakeHost::new(availability(&[HostRequirementKind::FileIO]));
            host.seed_filesystem_response(Err(HostError::new(code, "must remain a host failure")));
            let result = execute(&checked, &host).await;
            assert!(
                result.value().is_none(),
                "{call} caught {code:?}: {:?}",
                result.outcome
            );
            assert!(!result.diagnostics.is_empty(), "{call}: {code:?}");
            assert_eq!(host.filesystem_requests().len(), 1, "{call}: {code:?}");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn filesystem_invalid_success_payload_is_not_catchable_as_io_error() {
    let checked = program("stat(path)");
    let host = FakeHost::new(availability(&[HostRequirementKind::FileIO]));
    host.seed_filesystem_response(Ok(FilesystemEntry::Bytes(b"not a stat".to_vec())));
    let result = execute(&checked, &host).await;
    assert!(result.value().is_none());
    assert!(!result.diagnostics.is_empty());
    assert_eq!(host.filesystem_requests().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn filesystem_io_projection_requires_checked_error_type_identity() {
    let mut checked = program("stat(path)");
    std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut checked).types)
        .known_std_types
        .filesystem_io_error = None;
    let host = FakeHost::new(availability(&[HostRequirementKind::FileIO]));
    host.seed_filesystem_response(Err(HostError::new(
        HostErrorCode::IoFailure,
        "missing file",
    )));
    let result = execute(&checked, &host).await;
    assert!(result.value().is_none());
    assert!(result.diagnostics.iter().any(|diagnostic| diagnostic.code
        == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingCheckedFact)));
    assert_eq!(host.filesystem_requests().len(), 1);
}

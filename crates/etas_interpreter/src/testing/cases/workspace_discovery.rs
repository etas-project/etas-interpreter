use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};
use etas_host::{BoundedDirectoryList, FilesystemEntry, WorkspacePathRef, WorkspaceRegionId};

fn program(body: &str) -> CheckedProject {
    checked_project(&format!(
        r#"module app.main;
import std.fs.{{Region, WorkspacePath, IOError, ListLimitError, root, relative_name, list_limited}};
import std.effects.Fs;
import std.runtime.checkpoint;
type ProjectRoot; impl ProjectRoot ~ Region;
{body}"#
    ))
}

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
async fn root_and_relative_name_preserve_generic_region_without_host_authority() {
    let checked = program(
        r#"
flow name<R ~ Region>() -> string ![] { let path: WorkspacePath<R> = root(); return relative_name(path); }
flow main() -> string ![] { return name<ProjectRoot>(); }
"#,
    );
    let host = FakeHost::new(HostServiceAvailability::default());
    let result = execute(&checked, &host).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&value::InterpValue::String(".".into()))
    );
    assert!(host.filesystem_requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn bounded_listing_host_overflow_is_a_typed_error_not_a_partial_list() {
    let checked = program(
        r#"
flow main() -> usize ![Error<IOError>] {
    return handle { let entries = list_limited(root<ProjectRoot>(), 2, 20); 0 } with {
        Error<ListLimitError>.raise(ListLimitError.LimitExceeded(entries, bytes)) => { finish entries + bytes; }
    };
}"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::FileIO]));
    host.seed_filesystem_response(Ok(FilesystemEntry::LimitedEntries(
        BoundedDirectoryList::LimitExceeded {
            max_entries: 2,
            max_bytes: 20,
        },
    )));
    let result = execute(&checked, &host).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::usize(22)));
    assert_eq!(host.filesystem_requests().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn listing_bounds_survive_source_handler_and_checkpoint_and_reject_tampering() {
    for (maximum, expected) in [(1, 9), (2, 1)] {
        let checked = program(&format!(
            r#"
flow main() -> i32 ![] {{
    return handle {{
        let entries = list_limited(root<ProjectRoot>(), {maximum}, 20) with {{
            Fs.list<ProjectRoot>(_) => {{
                let paths: List<WorkspacePath<ProjectRoot>> = [std.result.unwrap(std.fs.path<ProjectRoot>("a")); std.result.unwrap(std.fs.path<ProjectRoot>("b"))];
                checkpoint("directory-handler"); resume paths;
            }}
        }};
        1
    }} with {{ Error<ListLimitError>.raise(_) => {{ finish 9; }} }};
}}"#
        ));
        let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
        let first = execute(&checked, &host).await;
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        assert_eq!(first.value(), Some(&value::InterpValue::i32(expected)));
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
        for field in ["max_entries", "max_bytes"] {
            let mut corrupt = artifact.clone();
            find_reducer(&mut corrupt).unwrap()[field] = serde_json::json!(999);
            assert!(
                checkpoint_from_json(&corrupt, &checked).is_err(),
                "accepted corrupt {field}"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_directory_host_responses_never_become_checked_results() {
    let region = WorkspaceRegionId::new("app.main.ProjectRoot").unwrap();
    for entry in [
        FilesystemEntry::Entries(vec![]),
        FilesystemEntry::LimitedEntries(BoundedDirectoryList::LimitExceeded {
            max_entries: 99,
            max_bytes: 20,
        }),
        FilesystemEntry::LimitedEntries(BoundedDirectoryList::Complete(vec![
            WorkspacePathRef::new(WorkspaceRegionId::new("app.Other").unwrap(), "a").unwrap(),
        ])),
        FilesystemEntry::LimitedEntries(BoundedDirectoryList::Complete(vec![
            WorkspacePathRef::new(region, "nested/a").unwrap(),
        ])),
    ] {
        let checked = program(
            "flow main() -> List<WorkspacePath<ProjectRoot>> ![Error<IOError>, Error<ListLimitError>] { return list_limited(root<ProjectRoot>(), 2, 20); }",
        );
        let host = FakeHost::new(availability(&[HostRequirementKind::FileIO]));
        host.seed_filesystem_response(Ok(entry));
        let result = execute(&checked, &host).await;
        assert!(!result.diagnostics.is_empty());
        assert!(result.value().is_none());
    }
}

fn find_reducer(value: &mut serde_json::Value) -> Option<&mut serde_json::Value> {
    if value.get("kind").and_then(serde_json::Value::as_str) == Some("directory_list_reduction") {
        return Some(value);
    }
    match value {
        serde_json::Value::Object(fields) => fields.values_mut().find_map(find_reducer),
        serde_json::Value::Array(values) => values.iter_mut().find_map(find_reducer),
        _ => None,
    }
}

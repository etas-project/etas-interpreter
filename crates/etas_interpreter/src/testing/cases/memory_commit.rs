use super::super::*;
use crate::testing::host::TestMemoryBackend;

#[tokio::test(flavor = "current_thread")]
async fn memory_write_receipt_wrong_target_is_unknown_and_not_retried() {
    let checked = checked_project(
        r#"
module app.main;
alias Schema = MemoryRegion<{ Papers: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "wrong-target", store = "test");
flow main() -> unit {
    retry limit Attempts(3) { Memory.Papers.put("key", "value"); }
}
"#,
    );
    for other_store in [false, true] {
        let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
        host.storage = Some(TestMemoryBackend::WrongWriteTarget {
            client: etas_host::InMemoryMemoryClient::new(),
            other_store,
        });
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                Vec::new(),
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("inconsistent target")),
            "{:?}",
            result.diagnostics
        );
        assert!(result.value().is_none());
        assert_eq!(host.memory_call_count(), 1);
        assert!(result.events.iter().any(|event| matches!(
            event,
            WorkflowEvent::StorageWrite {
                evidence: etas_host::StorageWriteEvidence {
                    status: etas_host::CommitStatus::Unknown,
                    ..
                },
                ..
            }
        )));
    }
}
use etas_host::MemoryClient;

#[derive(Debug, Default)]
pub(super) struct StopOnStorageCommit(std::sync::Mutex<Option<crate::api::RunControl>>);
impl StopOnStorageCommit {
    pub(super) fn arm(&self, control: crate::api::RunControl) {
        *self.0.lock().unwrap() = Some(control);
    }
}
impl crate::api::RunEventObserver for StopOnStorageCommit {
    fn observe(&self, event: &WorkflowEvent) {
        if matches!(
            event,
            WorkflowEvent::StorageWrite {
                evidence: etas_host::StorageWriteEvidence {
                    status: etas_host::CommitStatus::Committed { .. },
                    ..
                },
                ..
            }
        ) {
            self.0
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .stop(etas_host::execution::CancellationReason::Requested)
                .unwrap();
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_after_commit_retains_evidence_without_executing_next_statement() {
    let checked = checked_project(
        r#"
module app.main;
alias Schema = MemoryRegion<{ Papers: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "commit-cancel", store = "test");
flow main() -> unit {
    Memory.Papers.put("first", "committed");
    Memory.Papers.put("second", "must not execute");
}
"#,
    );
    let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.storage = Some(TestMemoryBackend::Volatile(
        etas_host::InMemoryMemoryClient::new(),
    ));
    let observer = std::sync::Arc::new(StopOnStorageCommit::default());
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        Vec::new(),
        &host,
        RunOptions {
            event_observer: Some(observer.clone()),
            ..Default::default()
        },
    );
    observer.arm(invocation.control());
    let result = invocation.execute().await.unwrap();
    assert!(
        matches!(result.outcome, crate::api::RunOutcome::Cancelled(_)),
        "{:?}",
        result.outcome
    );
    assert_eq!(host.memory_call_count(), 1);
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::StorageWrite {
            evidence: etas_host::StorageWriteEvidence {
                status: etas_host::CommitStatus::Committed { .. },
                ..
            },
            ..
        }
    )));
    assert!(
        result
            .termination
            .operations()
            .iter()
            .any(|operation| matches!(
                operation.outcome(),
                Some(etas_host::execution::ExternalOutcome::StorageWrite(
                    etas_host::StorageWriteEvidence {
                        status: etas_host::CommitStatus::Committed { .. },
                        ..
                    }
                ))
            ))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_replays_sqlite_write_with_original_operation_and_receipt() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};
alias Schema = MemoryRegion<{ Papers: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "receipt-resume", store = "test");
flow main() -> unit {
    checkpoint("before");
    Memory.Papers.put("key", "value");
    checkpoint("after");
}
"#,
    );
    let workspace = etas_host::TestWorkspace::create("receipt-resume").unwrap();
    let path = workspace.path().join("memory.db");
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    host.storage = Some(TestMemoryBackend::Sqlite(
        etas_host::SqliteMemoryClient::open(&path).unwrap(),
    ));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.checkpoints.len(), 2);
    assert!(first.checkpoints[0].storage.writes.is_empty());
    let committed = &first.checkpoints[1].storage.writes[0];
    assert!(matches!(
        committed.evidence.status,
        etas_host::CommitStatus::Committed { .. }
    ));
    let sources = [std::path::PathBuf::from("memory-receipt.es")];
    let artifact =
        crate::api::codec::checkpoint_artifact_json(&sources, "main", &first.checkpoints[0])
            .unwrap();
    let restored = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    drop(host);
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    host.storage = Some(TestMemoryBackend::Sqlite(
        etas_host::SqliteMemoryClient::open(&path).unwrap(),
    ));
    let resumed = Interpreter
        .resume_checkpoint(&checked, &restored, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        host.memory_call_count(),
        1,
        "resume must dispatch with an idempotency key, not skip through a fake cached result"
    );
    assert_eq!(
        resumed.checkpoints[0].storage.writes[0].evidence,
        committed.evidence
    );
    assert_ne!(
        resumed.checkpoints[0].host_state.trace.trace_id,
        first.checkpoints[1].host_state.trace.trace_id
    );
    let mut tampered =
        crate::api::codec::checkpoint_artifact_json(&sources, "main", &first.checkpoints[1])
            .unwrap();
    tampered["checkpoint"]["storage"]["writes"][0]["request"] = 999.into();
    assert!(
        crate::api::codec::checkpoint_from_json(&tampered, &checked)
            .unwrap_err()
            .message()
            .contains("boundary occurrence")
    );
    let mut old = artifact;
    old["schema"] = "etas.cli.interpreter-checkpoint.v19".into();
    assert!(
        crate::api::codec::checkpoint_from_json(&old, &checked)
            .unwrap_err()
            .message()
            .contains("unsupported checkpoint artifact schema")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn memory_clear_does_not_delete_concurrent_replacements() {
    let checked = checked_project(
        r#"
module app.main;
alias Schema = MemoryRegion<{ Papers: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "clear-race", store = "test");
flow main() -> string {
    Memory.Papers.put("key", "original");
    return handle {
        Memory.Papers.clear();
        return "cleared";
    } with {
        Error<MemoryConflict>.raise(_) => { finish "conflict"; }
    };
}
"#,
    );
    let client = etas_host::InMemoryMemoryClient::new();
    let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.storage = Some(TestMemoryBackend::ReplaceBeforeDelete(client.clone()));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::String("conflict".to_owned()))
    );
    let stored = client
        .execute(etas_host::MemoryRequest {
            id: HostRequestId(42),
            store: etas_host::StoreRef {
                region: etas_host::MemoryRegionRef {
                    stable_id: "clear-race".to_owned(),
                    schema_fingerprint: None,
                },
                path: vec!["Papers".to_owned()],
            },
            operation: etas_host::MemoryOperation::Get {
                key: HostValue::String("key".to_owned()),
            },
            authority: AuthorityContext::deny_all(),
            trace: TraceContext::root(TraceId(1)),
            budget: Default::default(),
        })
        .await
        .unwrap()
        .result
        .unwrap();
    assert!(
        matches!(stored, etas_host::MemoryResult::Value { value: HostValue::String(ref value), .. } if value == "concurrent replacement")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lost_memory_write_acknowledgement_is_not_retried() {
    let checked = checked_project(
        r#"
module app.main;
alias Schema = MemoryRegion<{ Papers: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "unknown", store = "test");
flow main() -> unit {
    retry limit Attempts(3) { Memory.Papers.put("key", "value"); }
}
"#,
    );
    for inner_error in [true, false] {
        let client = etas_host::InMemoryMemoryClient::new();
        let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
        host.storage = Some(TestMemoryBackend::LostWriteResponse {
            client: client.clone(),
            inner_error,
        });
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                Vec::new(),
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("commit outcome is unknown")),
            "{:?}",
            result.diagnostics
        );
        assert_eq!(
            host.memory_call_count(),
            1,
            "unknown commit must not enter retry"
        );
        let stored = client
            .execute(etas_host::MemoryRequest {
                id: HostRequestId(42),
                store: etas_host::StoreRef {
                    region: etas_host::MemoryRegionRef {
                        stable_id: "unknown".to_owned(),
                        schema_fingerprint: None,
                    },
                    path: vec!["Papers".to_owned()],
                },
                operation: etas_host::MemoryOperation::Get {
                    key: HostValue::String("key".to_owned()),
                },
                authority: AuthorityContext::deny_all(),
                trace: TraceContext::root(TraceId(1)),
                budget: Default::default(),
            })
            .await
            .unwrap()
            .result
            .unwrap();
        assert!(
            matches!(stored, etas_host::MemoryResult::Value { value: HostValue::String(ref value), .. } if value == "value")
        );
    }
}

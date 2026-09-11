use super::super::*;
use crate::testing::host::TestMemoryBackend;

fn source(body: &str, output: &str) -> String {
    format!(
        r#"
module app.main;
import std.memory.{{prepare_put, prepare_delete, operation_ref, commit, reconcile, Missing, Any}};
import std.runtime.checkpoint;
alias Schema = MemoryRegion<{{ Items: Store<string, string> }}>;
let Region = std.memory.region<Schema>(stable_id = "source-commit", store = "test");
flow submit<Key, Value>(intent: MemoryWriteIntent<Key, Value>) -> WriteOutcome<MemoryWriteReceipt<Key>, MemoryWriteRejection> {{
    return commit(intent);
}}
flow main() -> {output} {{ {body} }}
"#
    )
}

#[tokio::test(flavor = "current_thread")]
async fn source_commit_cancellation_keeps_receipt_and_stops_before_next_write() {
    let checked = checked_project(&source(
        r#"
        commit(prepare_put(Region.Items, "first", "committed", Missing));
        commit(prepare_put(Region.Items, "second", "must-not-execute", Missing));
        return;
    "#,
        "unit",
    ));
    let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.storage = Some(TestMemoryBackend::Volatile(
        etas_host::InMemoryMemoryClient::new(),
    ));
    let observer = std::sync::Arc::new(super::memory_commit::StopOnStorageCommit::default());
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
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
async fn source_commit_reconcile_and_replay_preserve_the_prepared_operation() {
    let checked = checked_project(&source(
        r#"
        let intent = prepare_put(Region.Items, "key", "secret-payload", Missing);
        let op = operation_ref(intent);
        checkpoint("prepared");
        let first = submit(intent);
        let repeated = submit(intent);
        let found = reconcile(Region.Items, op);
        checkpoint("written");
        let committed = match first {
            WriteOutcome.Committed(receipt) => receipt.operation.key == op.key && receipt.target.key == "key",
            _ => false,
        };
        let identical = match repeated {
            WriteOutcome.Committed(receipt) => match first {
                WriteOutcome.Committed(original) => match receipt.change {
                    MemoryWriteChange.Written(version) => match original.change {
                        MemoryWriteChange.Written(before) => version.opaque == before.opaque,
                        _ => false,
                    },
                    _ => false,
                },
                _ => false,
            },
            _ => false,
        };
        let reconciled = match found {
            ReconcileResult.Found(ConfirmedOutcome.Committed(receipt)) => receipt.operation.fingerprint == op.fingerprint,
            _ => false,
        };
        return committed && identical && reconciled;
    "#,
        "bool",
    ));
    for sqlite in [false, true] {
        let workspace = etas_host::TestWorkspace::create("source-commit").unwrap();
        let mut host = FakeHost::new(availability(&[
            HostRequirementKind::DurableMemory,
            HostRequirementKind::Checkpoint,
        ]));
        host.storage = Some(if sqlite {
            TestMemoryBackend::Sqlite(
                etas_host::SqliteMemoryClient::open(workspace.path().join("memory.db")).unwrap(),
            )
        } else {
            TestMemoryBackend::Volatile(etas_host::InMemoryMemoryClient::new())
        });
        let first = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![],
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(
            first.diagnostics.is_empty(),
            "{sqlite}: {:?}",
            first.diagnostics
        );
        assert_eq!(first.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(host.memory_call_count(), 3);
        let traces = first
            .events
            .iter()
            .filter_map(|event| match event {
                WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted {
                    kind: etas_host::HostRequestKind::Memory,
                    metadata,
                    ..
                }) => Some(metadata),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(traces.len(), 3);
        assert_eq!(traces[0].qualified_action, "Memory.write");
        assert_eq!(traces[2].qualified_action, "Memory.read");
        assert_eq!(traces[0].payload_digest, traces[1].payload_digest);
        assert_ne!(traces[0].payload_digest, traces[2].payload_digest);
        assert!(
            traces
                .iter()
                .flat_map(|metadata| &metadata.fields)
                .all(
                    |field| field.sensitivity == etas_host::HostTraceFieldSensitivity::Public
                        || field.value.is_none()
                )
        );
        assert!(!format!("{traces:?}").contains("secret-payload"));
        assert!(first.checkpoints[0].storage.writes.is_empty());
        let evidence = &first.checkpoints[1].storage.writes[0].evidence;
        let encoded = crate::api::codec::checkpoint_artifact_json(
            &["source-commit.es".into()],
            "main",
            &first.checkpoints[0],
        )
        .unwrap();
        let checkpoint = crate::api::codec::checkpoint_from_json(&encoded, &checked).unwrap();
        let resumed = Interpreter
            .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(
            host.memory_call_count(),
            6,
            "restore must query the backend, not cached interpreter success"
        );
        assert_eq!(&resumed.checkpoints[0].storage.writes[0].evidence, evidence);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn source_reconcile_recovers_unknown_without_reexecuting_the_write() {
    let checked = checked_project(&source(
        r#"
        let intent = prepare_put(Region.Items, "key", "value", Missing);
        let op = operation_ref(intent);
        let absent = match reconcile(Region.Items, op) {
            ReconcileResult.Unresolved => true, _ => false,
        };
        let uncertain = match commit(intent) {
            WriteOutcome.Unknown(actual) => actual.key == op.key, _ => false,
        };
        checkpoint("unknown-memory-write");
        return absent && uncertain && match reconcile(Region.Items, op) {
            ReconcileResult.Found(ConfirmedOutcome.Committed(receipt)) => receipt.operation == op,
            _ => false,
        };
    "#,
        "bool",
    ));
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Checkpoint,
    ]));
    host.storage = Some(TestMemoryBackend::LostWriteResponse {
        client: etas_host::InMemoryMemoryClient::new(),
        inner_error: false,
    });
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(host.memory_call_count(), 3);
    let actions = result
        .events
        .iter()
        .filter_map(|event| match event {
            WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted {
                kind: etas_host::HostRequestKind::Memory,
                metadata,
                ..
            }) => Some(metadata.qualified_action.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(actions, ["Memory.read", "Memory.write", "Memory.read"]);
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &["unknown-memory-write.es".into()],
        "main",
        &result.checkpoints[0],
    )
    .unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(
        host.memory_call_count(),
        4,
        "resume must only query the receipt"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn source_reconcile_rejects_invalid_reference_before_host_dispatch() {
    let checked = checked_project(&source(
        r#"
        let bad = StorageOperationRef { key = "not-an-operation", fingerprint = "invalid" };
        let captured: Result<ReconcileResult<MemoryWriteReceipt<string>, MemoryWriteRejection>, StorageError> = reconcile(Region.Items, bad)?;
        return match captured { Err(error) => error.code == "InvalidRequest", _ => false };
    "#,
        "bool",
    ));
    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(host.memory_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn source_unknown_commit_is_a_typed_outcome_and_never_retried() {
    let checked = checked_project(&source(
        r#"
        let intent = prepare_put(Region.Items, "key", "payload", Any);
        let op = operation_ref(intent);
        retry limit Attempts(3) {
            return match commit(intent) {
                WriteOutcome.Unknown(actual) => actual.key == op.key && actual.fingerprint == op.fingerprint,
                _ => false,
            };
        }
        return false;
    "#,
        "bool",
    ));
    for backend in [
        TestMemoryBackend::LostWriteResponse {
            client: etas_host::InMemoryMemoryClient::new(),
            inner_error: false,
        },
        TestMemoryBackend::LostWriteResponse {
            client: etas_host::InMemoryMemoryClient::new(),
            inner_error: true,
        },
        TestMemoryBackend::WrongWriteTarget {
            client: etas_host::InMemoryMemoryClient::new(),
            other_store: true,
        },
        TestMemoryBackend::WrongWriteTarget {
            client: etas_host::InMemoryMemoryClient::new(),
            other_store: false,
        },
    ] {
        let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
        host.storage = Some(backend);
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![],
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
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

#[tokio::test(flavor = "current_thread")]
async fn source_commit_policy_denial_is_typed_and_precedes_storage_dispatch() {
    let checked = checked_project(&source(
        r#"
        let intent = prepare_put(Region.Items, "key", "payload", Any);
        let captured: Result<WriteOutcome<MemoryWriteReceipt<string>, MemoryWriteRejection>, StorageError> = commit(intent)?;
        return match captured {
            Err(error) => error.code == "AuthorityDenied",
            _ => false,
        };
    "#,
        "bool",
    ));
    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_policy_decision(PolicyDecision::Deny {
        reason: "deny storage".into(),
    });
    let mut options = RunOptions::default();
    options.host_context.authority.policy =
        boundary_policy_context(HostValue::String("storage-policy".into()));
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
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(host.policy_call_count(), 1);
    assert_eq!(host.memory_call_count(), 0);
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::StorageWrite { .. }))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn source_delete_returns_tombstone_and_conflict_preserves_operation() {
    let checked = checked_project(&source(
        r#"
        let first = commit(prepare_put(Region.Items, "key", "value", Missing));
        let rejected = prepare_put(Region.Items, "key", "different", Missing);
        let op = operation_ref(rejected);
        let conflict = match commit(rejected) {
            WriteOutcome.NotCommitted(actual, MemoryWriteRejection.ConditionConflict(_, _)) => actual.key == op.key,
            _ => false,
        };
        let deletion = prepare_delete(Region.Items, "key", Any);
        let deleted = match commit(deletion) {
            WriteOutcome.Committed(receipt) => match receipt.change {
                MemoryWriteChange.Deleted(tombstone) => tombstone.opaque != "",
                _ => false,
            },
            _ => false,
        };
        return conflict && deleted && match reconcile(Region.Items, operation_ref(deletion)) {
            ReconcileResult.Found(ConfirmedOutcome.Committed(receipt)) => match receipt.change {
                MemoryWriteChange.Deleted(_) => true, _ => false,
            },
            _ => false,
        };
    "#,
        "bool",
    ));
    for sqlite in [false, true] {
        let workspace = etas_host::TestWorkspace::create("source-delete").unwrap();
        let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
        host.storage = Some(if sqlite {
            TestMemoryBackend::Sqlite(
                etas_host::SqliteMemoryClient::open(workspace.path().join("memory.db")).unwrap(),
            )
        } else {
            TestMemoryBackend::Volatile(etas_host::InMemoryMemoryClient::new())
        });
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![],
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(host.memory_call_count(), 4);
    }
}

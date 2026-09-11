use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn storage_outcome_variants_keep_operation_and_rejection_payloads() {
    let checked = checked_project(
        r#"
module app.main;
import std.memory.{prepare_delete, operation_ref, Any};
alias Schema = MemoryRegion<{ Items: Store<string, string> }>;
let Region = std.memory.region<Schema>(stable_id = "outcome-region", store = "outcome-store");
flow main() -> bool ![Error<StorageError>] {
    let op = operation_ref(prepare_delete(Region.Items, "key", Any));
    let reason = StorageError { code = "conflict", message = "unchanged" };
    let unknown: WriteOutcome<string, StorageError> = WriteOutcome.Unknown(op);
    let rejected: WriteOutcome<string, StorageError> = WriteOutcome.NotCommitted(op, reason);
    let confirmed: ConfirmedOutcome<string, StorageError> = ConfirmedOutcome.NotCommitted(op, reason);
    let uncertain_ok = match unknown {
        WriteOutcome.Unknown(operation) => operation.key == op.key,
        _ => false,
    };
    let rejected_ok = match rejected {
        WriteOutcome.NotCommitted(operation, error) => operation.fingerprint == op.fingerprint && error.code == "conflict",
        _ => false,
    };
    let confirmed_ok = match confirmed {
        ConfirmedOutcome.NotCommitted(operation, error) => operation.key == op.key && error.message == "unchanged",
        _ => false,
    };
    return uncertain_ok && rejected_ok && confirmed_ok;
}
"#,
    );
    let host = FakeHost::new(HostServiceAvailability::default());
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
async fn storage_outcome_adt_constructors_match_and_survive_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.memory.{WriteOutcome, ConfirmedOutcome, ReconcileResult};
import std.runtime.checkpoint;
flow classify(value: ReconcileResult<string, i32>) -> string {
    return match value {
        ReconcileResult.Found(receipt) => match receipt {
            ConfirmedOutcome.Committed(text) => text,
            ConfirmedOutcome.NotCommitted(_, _) => "rejected",
        },
        ReconcileResult.Unresolved() => "unresolved",
        ReconcileResult.Expired() => "expired",
    };
}
flow payload() -> string {
    checkpoint("constructor-argument");
    return "committed";
}
flow main() -> bool {
    let receipt: ConfirmedOutcome<string, i32> = ConfirmedOutcome.Committed(payload());
    let found: ReconcileResult<string, i32> = ReconcileResult.Found(receipt);
    let unresolved: ReconcileResult<string, i32> = ReconcileResult.Unresolved();
    let expired: ReconcileResult<string, i32> = ReconcileResult.Expired();
    let write: WriteOutcome<string, i32> = WriteOutcome.Committed("written");
    checkpoint("outcomes");
    return classify(found) == "committed" && classify(unresolved) == "unresolved"
        && classify(expired) == "expired" && match write {
            WriteOutcome.Committed(text) => text == "written", _ => false,
        };
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
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
    assert_eq!(result.checkpoints.len(), 2);
    for snapshot in &result.checkpoints {
        let artifact =
            crate::api::codec::checkpoint_artifact_json(&["outcome.es".into()], "main", snapshot)
                .unwrap();
        let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
        let resumed = Interpreter
            .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(host.memory_call_count(), 0);
    }
}

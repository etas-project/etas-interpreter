use super::super::*;

fn preparation_source(body: &str, output: &str) -> String {
    format!(
        r#"
module app.main;
import std.memory.{{prepare_put, prepare_delete, operation_ref, Any, Missing, Exists, Match}};
import std.runtime.checkpoint;
alias Schema = MemoryRegion<{{ Items: Store<string, string> }}>;
let Region = std.memory.region<Schema>(stable_id = "intent-region", store = "intent-test");
flow main() -> {output} ![Error<StorageError>] {{ {body} }}
"#
    )
}

#[tokio::test(flavor = "current_thread")]
async fn source_prepare_intent_checkpoints_without_storage_access_and_restores_same_identity() {
    let checked = checked_project(&preparation_source(
        r#"
        let intent = prepare_put(Region.Items, "key", "payload", Missing);
        checkpoint("before-commit");
        return intent;
    "#,
        "MemoryWriteIntent<string, string>",
    ));
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let summary = checked
        .effects
        .item_effects
        .get(&checked.entry.unwrap())
        .unwrap();
    assert!(
        summary.requested_actions.effects.is_empty(),
        "preparation must not claim a storage action: {summary:?}"
    );
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
    let Some(InterpValue::MemoryWriteIntent(value)) = result.value() else {
        panic!("typed intent")
    };
    let operation = value.intent().operation_ref().clone();
    assert!(result.checkpoints[0].storage.writes.is_empty());
    assert_eq!(host.memory_call_count(), 0);
    let encoded = crate::api::codec::value_json(result.value().unwrap());
    assert_eq!(
        crate::api::codec::value_from_json(&encoded).unwrap(),
        *result.value().unwrap()
    );
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &["intent.es".into()],
        "main",
        &result.checkpoints[0],
    )
    .unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let mut obsolete = artifact.clone();
    obsolete["schema"] = "etas.cli.interpreter-checkpoint.v25".into();
    assert!(
        crate::api::codec::checkpoint_from_json(&obsolete, &checked)
            .unwrap_err()
            .message()
            .contains("unsupported checkpoint artifact schema")
    );
    let mut malformed = encoded.clone();
    malformed["intent"] = value.encoded().replace("payload", "changed").into();
    assert!(
        crate::api::codec::value_from_json(&malformed).is_err(),
        "mutation without its bound fingerprint must fail"
    );
    for field in ["ty", "key_type", "value_type", "intent"] {
        let mut missing = encoded.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(crate::api::codec::value_from_json(&missing).is_err());
    }
    malformed = encoded.clone();
    malformed["authority"] = serde_json::json!({"allow":true});
    assert!(crate::api::codec::value_from_json(&malformed).is_err());
    malformed = encoded.clone();
    malformed["ty"] = value.key_type.0.into();
    let bad_value = crate::api::codec::value_from_json(&malformed).unwrap();
    let mut injected = artifact.clone();
    injected["checkpoint"]["args"] = serde_json::json!([malformed]);
    assert!(crate::api::codec::checkpoint_from_json(&injected, &checked).is_err());
    let mut in_memory = checkpoint.clone();
    in_memory.args = vec![bad_value];
    let rejected = Interpreter
        .resume_checkpoint(&checked, &in_memory, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(matches!(
        rejected.outcome,
        crate::api::RunOutcome::Failed(crate::api::RunFailure::RestoreRejected { .. })
    ));
    assert!(rejected.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("checkpoint state validation failed")
    }));
    assert!(rejected.events.is_empty());
    assert!(rejected.checkpoints.is_empty());
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    let Some(InterpValue::MemoryWriteIntent(actual)) = resumed.value() else {
        panic!("restored intent")
    };
    assert_eq!(actual.intent().operation_ref(), &operation);
    assert_eq!(resumed.value(), result.value());
    assert_eq!(host.memory_call_count(), 0);
}

#[test]
fn source_cannot_construct_or_retype_an_opaque_write_intent() {
    for body in [
        "return MemoryWriteIntent<string, string>();",
        "return {};",
        "return [];",
        "return prepare_put(Region.Items, true, \"payload\", Any);",
    ] {
        let output = etas_frontend::Frontend.check(etas_frontend::SourceInput {
            id: etas_core::SourceId(1),
            path: None,
            text: preparation_source(body, "MemoryWriteIntent<string, string>"),
            kind: etas_frontend::SourceKind::SourceProjectFile,
        });
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| matches!(d.code, etas_core::DiagnosticCode::Type(_))),
            "{body}: {:?}",
            output.diagnostics
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn source_preparation_reports_typed_size_failure_without_storage_dispatch() {
    let source = preparation_source(
        "return prepare_put(Region.Items, \"key\", payload, Any)?;",
        "Result<MemoryWriteIntent<string, string>, StorageError>",
    )
    .replace("flow main()", "flow main(payload: string)");
    let checked = checked_project(&source);
    let host = FakeHost::new(availability(&[]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::String("x".repeat(
                etas_host::StorageLimits::default().max_value_bytes + 1,
            ))],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(InterpValue::Variant { name, fields }) = result.value() else {
        panic!("Result value")
    };
    assert_eq!(name, "Err");
    let [InterpValue::Nominal { value, .. }] = fields.as_slice() else {
        panic!("nominal StorageError")
    };
    let InterpValue::Record(fields) = value.as_ref() else {
        panic!("StorageError representation")
    };
    assert_eq!(
        fields
            .borrow()
            .iter()
            .find(|(name, _)| name == "code")
            .map(|(_, value)| value),
        Some(&InterpValue::String(
            etas_host::HostErrorCode::BudgetExceeded.as_str().into()
        ))
    );
    assert_eq!(host.memory_call_count(), 0);
    assert!(result.checkpoints.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn source_prepare_delete_exposes_stable_typed_operation_ref() {
    let checked = checked_project(&preparation_source(
        r#"
        let intent = prepare_delete(Region.Items, "key", Any);
        let before = operation_ref(intent);
        checkpoint("prepared-delete");
        let after = operation_ref(intent);
        return before.key == after.key && before.fingerprint == after.fingerprint;
    "#,
        "bool",
    ));
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
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
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(host.memory_call_count(), 0);
}

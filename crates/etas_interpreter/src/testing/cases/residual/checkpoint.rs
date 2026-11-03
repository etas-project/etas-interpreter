use super::*;

pub(super) fn checkpoint_source(checkpoint_in_guard: bool) -> String {
    let before_send = if checkpoint_in_guard {
        ""
    } else {
        "checkpoint(\"ready\");"
    };
    let in_guard = if checkpoint_in_guard {
        "checkpoint(\"pending\");"
    } else {
        ""
    };
    format!(
        r#"
module app.main;
import std.runtime.checkpoint;
effect Gate {{ action check() -> unit; action send() -> unit; }}
spec Safe: trace = Gate.check >> Gate.send;
flow main() -> i32 ![] ~ Safe {{
    var sent = 0;
    handle {{
        perform Gate.check();
        {before_send}
        perform Gate.send();
    }} with {{
        Gate.check() => {{ {in_guard} resume (); }}
        Gate.send() => {{ sent = sent + 1; resume (); }}
    }};
    return sent;
}}
"#
    )
}

pub(super) async fn first_checkpoint(
    checked: &etas_frontend::CheckedProject,
) -> crate::api::InterpreterCheckpoint {
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let first = run(checked, &host).await;
    assert!(first.diagnostics.is_empty(), "{:#?}", first.diagnostics);
    assert_eq!(first.value(), Some(&value::InterpValue::i32(1)));
    assert_eq!(first.checkpoints.len(), 1);
    first.checkpoints.into_iter().next().unwrap()
}

async fn resume(
    checked: &etas_frontend::CheckedProject,
    checkpoint: &crate::api::InterpreterCheckpoint,
) -> crate::api::RunResult {
    Interpreter
        .resume_checkpoint(
            checked,
            checkpoint,
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_preserves_completed_guard_evidence_without_replaying_it() {
    let checked = residual_checked(&checkpoint_source(false));
    let checkpoint = first_checkpoint(&checked).await;
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &[std::path::PathBuf::from("src/app/main.es")],
        "main",
        &checkpoint,
    )
    .unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = resume(&checked, &checkpoint).await;
    assert!(resumed.diagnostics.is_empty(), "{:#?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&value::InterpValue::i32(1)));
    assert!(
        resumed.checkpoints.is_empty(),
        "the guard/checkpoint must not replay"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_removing_a_required_scope_and_its_owner_frame() {
    let checked = residual_checked(&checkpoint_source(false));
    let mut checkpoint = first_checkpoint(&checked).await;
    assert_eq!(checkpoint.machine.residual.scopes.len(), 1);
    let before = checkpoint.machine.frames.len();
    checkpoint.machine.frames.retain(|frame| {
        !matches!(
            frame,
            crate::orchestration::MachineFrameSnapshot::ResidualScope { .. }
        )
    });
    assert_eq!(checkpoint.machine.frames.len(), before - 1);
    checkpoint.machine.residual.scopes.clear();
    let result = resume(&checked, &checkpoint).await;
    assert!(
        result.value().is_none(),
        "a checked scope must not be optional"
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("required residual scope")),
        "{:#?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_decoder_rejects_removing_the_required_scope_and_frame_together() {
    let checked = residual_checked(&checkpoint_source(false));
    let mut checkpoint = first_checkpoint(&checked).await;
    checkpoint.machine.frames.retain(|frame| {
        !matches!(
            frame,
            crate::orchestration::MachineFrameSnapshot::ResidualScope { .. }
        )
    });
    checkpoint.machine.residual.scopes.clear();
    let artifact = crate::api::codec::checkpoint_artifact_json(&[], "main", &checkpoint).unwrap();
    let error = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap_err();
    assert!(
        error.message().contains("required residual scope"),
        "{error}"
    );
}

const RECURSIVE_SCOPE_CHECKPOINT: &str = r#"
module app.main;
import std.runtime.checkpoint;
effect Gate { action check() -> unit; action send() -> unit; }
spec Safe: trace = Gate.check >> Gate.send;
flow guarded(depth: i32) -> unit ![Gate.check, Gate.send] ~ Safe {
    perform Gate.check();
    if depth > 0 {
        guarded(depth - 1);
    } else {
        checkpoint("inner");
    }
    perform Gate.send();
}
flow main() -> i32 {
    var sent = 0;
    handle { guarded(2); } with {
        Gate.check() => { resume (); }
        Gate.send() => { sent = sent + 1; resume (); }
    };
    return sent;
}
"#;

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_removing_scope_of_a_tail_position_caller() {
    let checked = residual_checked(
        r#"
module app.main;
import std.runtime.checkpoint;
effect Gate { action check() -> unit; action send() -> unit; }
spec Safe: trace = Gate.check >> Gate.send;
flow leaf() -> unit ![Gate.send] {
    checkpoint("leaf");
    perform Gate.send();
}
flow guarded() -> unit ![Gate.check, Gate.send] ~ Safe {
    perform Gate.check();
    return leaf();
}
flow main() -> i32 {
    var sent = 0;
    handle { guarded(); } with {
        Gate.check() => { resume (); }
        Gate.send() => { sent = sent + 1; resume (); }
    };
    return sent;
}
"#,
    );
    let mut checkpoint = first_checkpoint(&checked).await;
    assert_eq!(checkpoint.machine.residual.scopes.len(), 1);
    let valid = resume(&checked, &checkpoint).await;
    assert!(valid.diagnostics.is_empty(), "{:#?}", valid.diagnostics);
    assert_eq!(valid.value(), Some(&value::InterpValue::i32(1)));
    checkpoint.machine.frames.retain(|frame| {
        !matches!(
            frame,
            crate::orchestration::MachineFrameSnapshot::ResidualScope { .. }
        )
    });
    checkpoint.machine.residual.scopes.clear();
    let invalid = resume(&checked, &checkpoint).await;
    assert!(
        invalid.value().is_none(),
        "a tail-position call must retain the caller's checked scope"
    );
    assert!(
        invalid
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("required residual scope")),
        "{:#?}",
        invalid.diagnostics
    );
}

async fn recursive_checkpoint() -> (
    etas_frontend::CheckedProject,
    crate::api::InterpreterCheckpoint,
) {
    let checked = residual_checked(RECURSIVE_SCOPE_CHECKPOINT);
    let result = run(
        &checked,
        &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(3)));
    assert_eq!(result.checkpoints.len(), 1);
    let checkpoint = result.checkpoints.into_iter().next().unwrap();
    assert_eq!(checkpoint.machine.residual.scopes.len(), 3);
    assert!(checkpoint.machine.residual.attempts.is_empty());
    (checked, checkpoint)
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_preserves_independent_recursive_scope_owners() {
    let (checked, checkpoint) = recursive_checkpoint().await;
    let artifact = crate::api::codec::checkpoint_artifact_json(&[], "main", &checkpoint).unwrap();
    let restored = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let result = resume(&checked, &restored).await;
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(3)));
    assert!(result.checkpoints.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_removing_callee_scopes_with_an_unguarded_entry() {
    let (checked, mut checkpoint) = recursive_checkpoint().await;
    checkpoint.machine.frames.retain(|frame| {
        !matches!(
            frame,
            crate::orchestration::MachineFrameSnapshot::ResidualScope { .. }
        )
    });
    checkpoint.machine.residual.scopes.clear();
    let result = resume(&checked, &checkpoint).await;
    assert!(result.value().is_none());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("required residual scope")),
        "{:#?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_swapping_recursive_scope_invocations() {
    use crate::orchestration::MachineFrameSnapshot;
    let (checked, mut checkpoint) = recursive_checkpoint().await;
    for frame in &mut checkpoint.machine.frames {
        if let MachineFrameSnapshot::ResidualScope { scope } = frame {
            *scope = match *scope {
                0 => 1,
                1 => 0,
                other => other,
            };
        }
    }
    let result = resume(&checked, &checkpoint).await;
    assert!(
        result.value().is_none(),
        "the same callable's invocations are not interchangeable"
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("residual scope invocation")),
        "{:#?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_preserves_pending_guard_until_its_handler_resumes() {
    let checked = residual_checked(&checkpoint_source(true));
    let checkpoint = first_checkpoint(&checked).await;
    assert_eq!(checkpoint.machine.residual.scopes.len(), 1);
    assert_eq!(checkpoint.machine.residual.attempts.len(), 1);
    assert_eq!(checkpoint.machine.residual.scopes[0].pending.len(), 1);
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &[std::path::PathBuf::from("src/app/main.es")],
        "main",
        &checkpoint,
    )
    .unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = resume(&checked, &checkpoint).await;
    assert!(resumed.diagnostics.is_empty(), "{:#?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&value::InterpValue::i32(1)));
    assert!(resumed.checkpoints.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_clearing_frame_scope_identity_without_clearing_monitor_state() {
    let checked = residual_checked(&checkpoint_source(false));
    let checkpoint = first_checkpoint(&checked).await;
    let mut artifact =
        crate::api::codec::checkpoint_artifact_json(&[], "main", &checkpoint).unwrap();
    let mut pending = vec![&mut *artifact];
    let mut changed = 0;
    while let Some(value) = pending.pop() {
        match value {
            serde_json::Value::Object(object) => {
                if object.contains_key("locals")
                    && object
                        .get("residual_invocation")
                        .is_some_and(|value| !value.is_null())
                {
                    object.insert("residual_invocation".into(), serde_json::Value::Null);
                    changed += 1;
                }
                pending.extend(object.values_mut());
            }
            serde_json::Value::Array(array) => pending.extend(array.iter_mut()),
            _ => {}
        }
    }
    assert!(changed > 0);
    let error = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap_err();
    assert!(
        error
            .message()
            .contains("required residual scope invocation"),
        "{error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_scope_identity_survives_a_second_resume_checkpoint() {
    let source = checkpoint_source(false).replace(
        "checkpoint(\"ready\");",
        "checkpoint(\"ready\"); checkpoint(\"again\");",
    );
    let checked = residual_checked(&source);
    let result = run(
        &checked,
        &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.checkpoints.len(), 2);
    let resumed = resume(&checked, &result.checkpoints[0]).await;
    assert!(resumed.diagnostics.is_empty(), "{:#?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&value::InterpValue::i32(1)));
    assert_eq!(resumed.checkpoints.len(), 1);
    let artifact =
        crate::api::codec::checkpoint_artifact_json(&[], "main", &resumed.checkpoints[0]).unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let again = resume(&checked, &checkpoint).await;
    assert!(again.diagnostics.is_empty(), "{:#?}", again.diagnostics);
    assert_eq!(again.value(), Some(&value::InterpValue::i32(1)));
    assert!(again.checkpoints.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_action_identities_survive_independent_recompilation() {
    for pending in [false, true] {
        let source = checkpoint_source(pending);
        let checked = residual_checked(&source);
        let checkpoint = first_checkpoint(&checked).await;
        let artifact =
            crate::api::codec::checkpoint_artifact_json(&[], "main", &checkpoint).unwrap();
        let residual = &artifact["checkpoint"]["machine"]["residual"];
        let entries = if pending {
            &residual["scopes"][0]["pending"]
        } else {
            &residual["scopes"][0]["outcomes"]
        };
        assert!(
            entries[0].get("effect").is_none(),
            "no process-local action IDs may cross the checkpoint boundary"
        );
        assert!(entries[0].get("action").is_some());
        for iteration in 0..16 {
            let recompiled = residual_checked(&source);
            let restored = crate::api::codec::checkpoint_from_json(&artifact, &recompiled).unwrap();
            let result = resume(&recompiled, &restored).await;
            assert!(
                result.diagnostics.is_empty(),
                "recompilation {iteration}, pending={pending}: {:#?}",
                result.diagnostics
            );
            assert_eq!(result.value(), Some(&value::InterpValue::i32(1)));
            assert!(result.checkpoints.is_empty());
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_unknown_canonical_action_identity() {
    let checked = residual_checked(&checkpoint_source(false));
    let mut checkpoint = first_checkpoint(&checked).await;
    let action = &mut checkpoint.machine.residual.scopes[0].outcomes[0].action;
    let etas_code::descriptor::EffectTargetInfo::Action { name, .. } = &mut action.target else {
        panic!("concrete action")
    };
    *name = "not_declared".into();
    let result = resume(&checked, &checkpoint).await;
    assert!(result.value().is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("absent from the checked scope footprint")),
        "{:#?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_attempt_with_a_different_pending_action_identity() {
    let checked = residual_checked(&checkpoint_source(true));
    let mut checkpoint = first_checkpoint(&checked).await;
    let action = &mut checkpoint.machine.residual.attempts[0].scopes[0].action;
    let etas_code::descriptor::EffectTargetInfo::Action { name, .. } = &mut action.target else {
        panic!("concrete action")
    };
    *name = "send".into();
    let result = resume(&checked, &checkpoint).await;
    assert!(result.value().is_none());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("pending action identity")),
        "{:#?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_outcome_that_postdates_its_scope() {
    let checked = residual_checked(&checkpoint_source(false));
    let mut checkpoint = first_checkpoint(&checked).await;
    let scope = &mut checkpoint.machine.residual.scopes[0];
    scope.outcomes[0].accepted.as_mut().unwrap().completed_at = scope.last_cursor.unwrap() + 1;
    let result = resume(&checked, &checkpoint).await;
    assert!(result.value().is_none());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("evidence postdates")),
        "{:#?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_reused_outcome_attempt_identity() {
    let checked = residual_checked(&checkpoint_source(false));
    let mut checkpoint = first_checkpoint(&checked).await;
    checkpoint.machine.residual.next_attempt = 0;
    let result = resume(&checked, &checkpoint).await;
    assert!(
        result.value().is_none(),
        "restoration must not reuse a completed attempt ID"
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("reuse an identity")),
        "{:#?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_rewound_outcome_cursor() {
    let checked = residual_checked(&checkpoint_source(false));
    let mut checkpoint = first_checkpoint(&checked).await;
    checkpoint.machine.residual.next_cursor =
        checkpoint.machine.residual.scopes[0].last_cursor.unwrap() - 1;
    let result = resume(&checked, &checkpoint).await;
    assert!(result.value().is_none());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("reuse a recorded event")),
        "{:#?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_lost_pending_outcome_correlation() {
    let checked = residual_checked(&checkpoint_source(true));
    let mut checkpoint = first_checkpoint(&checked).await;
    checkpoint.machine.residual.scopes[0].pending.clear();
    let result = resume(&checked, &checkpoint).await;
    assert!(result.value().is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("disagrees with its scope outcome")),
        "{:#?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn scope_cannot_complete_with_an_unsettled_guard_outcome() {
    use crate::control::residual::{
        ResidualExitKind, ResidualRunLog, ResidualScopeInstance, ResidualStateLimits,
    };
    let checked = residual_checked(&checkpoint_source(true));
    let checkpoint = first_checkpoint(&checked).await;
    let snapshot = &checkpoint.machine.residual.scopes[0];
    let prepared = crate::plan::build_plan(&checked, Default::default())
        .plan
        .unwrap();
    let scope = prepared
        .residual
        .scope(etas_hir::HirItemId(snapshot.item))
        .unwrap()
        .clone();
    let log = ResidualRunLog::new(8);
    let mut instance = ResidualScopeInstance::restore(
        snapshot,
        scope,
        checked.clone(),
        &log,
        ResidualStateLimits::default(),
    )
    .unwrap();
    assert_eq!(
        instance.finish(ResidualExitKind::Return).unwrap(),
        etas_code::residual::monitor::ScopeCheckResult::PendingEvidence
    );
    assert_eq!(log.exits().len(), 1);
    assert_eq!(log.exits()[0].unknown_outcomes, 1);
}

const CAPTURED_ERROR_CHECKPOINT: &str = r#"
module app.main;
import std.runtime.checkpoint;
effect Gate { action check() -> unit; action send() -> unit; }
spec Safe: trace = Gate.check >> Gate.send;
flow guarded(crash: bool) -> unit ![Error<i32>] ~ Safe {
    handle {
        perform Gate.check();
        if crash { perform Error<i32>.raise(1); }
        perform Gate.send();
    } with {
        Gate.check() => { resume (); }
        Gate.send() => { resume (); }
    };
}
flow main() -> i32 {
    let ignored = guarded(true)?;
    checkpoint("after-error");
    return 7;
}
"#;

async fn incomplete_checkpoint() -> (
    etas_frontend::CheckedProject,
    crate::api::InterpreterCheckpoint,
) {
    let checked = residual_checked(CAPTURED_ERROR_CHECKPOINT);
    let result = run(
        &checked,
        &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
    )
    .await;
    super::lifecycle::assert_incomplete(&result, "ErrorPropagation");
    assert_eq!(result.checkpoints.len(), 1, "{:#?}", result.diagnostics);
    let checkpoint = result.checkpoints.into_iter().next().unwrap();
    assert!(
        checkpoint.machine.residual.scopes.is_empty(),
        "the failed callee frame has already unwound"
    );
    assert_eq!(checkpoint.machine.residual.exits.unsatisfied, 1);
    (checked, checkpoint)
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_preserves_incomplete_exit_after_error_capture() {
    let (checked, checkpoint) = incomplete_checkpoint().await;
    let artifact = crate::api::codec::checkpoint_artifact_json(&[], "main", &checkpoint).unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = resume(&checked, &checkpoint).await;
    super::lifecycle::assert_incomplete(&resumed, "ErrorPropagation");
    assert_eq!(
        resumed
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code
                == DiagnosticCode::Analysis(AnalysisDiagnosticCode::TraceSpecResidualIncomplete))
            .count(),
        1
    );
    assert!(resumed.checkpoints.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_historical_failure_forged_as_accepting() {
    let (checked, mut checkpoint) = incomplete_checkpoint().await;
    checkpoint.machine.residual.exits.records[0].exit.result =
        crate::orchestration::ResidualScopeResultSnapshot::Accepting;
    let result = resume(&checked, &checkpoint).await;
    assert!(result.value().is_none());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("invalid record")),
        "{:#?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_rejects_missing_durable_exit_history() {
    let (checked, checkpoint) = incomplete_checkpoint().await;
    let mut artifact =
        crate::api::codec::checkpoint_artifact_json(&[], "main", &checkpoint).unwrap();
    artifact["checkpoint"]["machine"]["residual"]
        .as_object_mut()
        .unwrap()
        .remove("exits");
    let error = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap_err();
    assert!(error.message().contains("missing field `exits`"), "{error}");
}

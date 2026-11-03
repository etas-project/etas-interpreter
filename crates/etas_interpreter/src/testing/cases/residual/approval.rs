use super::*;
use crate::control::residual::{
    ResidualPreview, ResidualRunLog, ResidualScopeInstance, ResidualStateLimits,
};
use etas_code::residual::monitor::{ActionAttemptId, ScopeInvocationId, SemanticEventCursor};
use std::rc::Rc;

fn identity(kind: u8, counter: u8) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[0] = kind;
    bytes[15] = counter;
    bytes
}

fn checked_approval_scope() -> (
    etas_frontend::CheckedProject,
    std::sync::Arc<crate::plan::residual::PreparedResidualScope>,
    Rc<ResidualRunLog>,
    ResidualScopeInstance,
) {
    let checked = residual_checked(
        r#"
module app.main;
import std.effects.{Approval, Console};
import std.runtime.approval.ApprovalRequest;
spec Safe: trace = Approval.request >> Console.stdout_write;
flow main(request: ApprovalRequest) -> unit ![Approval.request, Console.stdout_write] ~ Safe {
    perform Approval.request(request);
    perform Console.stdout_write("sent");
    return;
}
"#,
    );
    let plan = crate::plan::build_plan(&checked, Default::default())
        .plan
        .unwrap();
    let prepared = plan.residual.scope(checked.entry.unwrap()).unwrap().clone();
    let log = ResidualRunLog::new(8);
    let scope = ResidualScopeInstance::activate(
        prepared.clone(),
        checked.clone(),
        ScopeInvocationId::from_bytes(identity(b's', 1)),
        &log,
        ResidualStateLimits::default(),
    )
    .unwrap();
    (checked, prepared, log, scope)
}

fn label(
    scope: &mut ResidualScopeInstance,
    checked: &etas_frontend::CheckedProject,
    name: &str,
) -> etas_code::contract::CanonicalEffect {
    let action = checked
        .effect_registry
        .standard_action_by_name(name)
        .unwrap();
    scope.label(&etas_effects::Effect::Action(action)).unwrap()
}

fn commit_approval(
    scope: &mut ResidualScopeInstance,
    action: etas_code::contract::CanonicalEffect,
    counter: u8,
    cursor: u64,
) {
    let attempt = ActionAttemptId::from_bytes(identity(b'a', counter));
    let ResidualPreview::Permit(deltas) = scope
        .preview(attempt, SemanticEventCursor(cursor), &action)
        .unwrap()
    else {
        panic!("approval request is permitted, not already accepted")
    };
    scope
        .commit(deltas, attempt, SemanticEventCursor(cursor), action)
        .unwrap();
    scope
        .record_outcome(
            attempt,
            crate::control::residual::ResidualOutcomeStatus::Accepted,
            SemanticEventCursor(cursor + 1),
        )
        .unwrap();
}

#[test]
fn resumed_approval_is_history_not_permission_and_explicit_new_approval_can_succeed() {
    let (checked, prepared, log, mut scope) = checked_approval_scope();
    let approval = label(&mut scope, &checked, "Approval.request");
    let send = label(&mut scope, &checked, "Console.stdout_write");
    commit_approval(&mut scope, approval.clone(), 1, 1);
    assert!(matches!(
        scope
            .preview(
                ActionAttemptId::from_bytes(identity(b'a', 2)),
                SemanticEventCursor(3),
                &send
            )
            .unwrap(),
        ResidualPreview::Permit(_)
    ));
    let original = scope.snapshot();
    let encoded = serde_json::to_value(&original).unwrap();
    let snapshot = serde_json::from_value(encoded).unwrap();
    let mut restored = ResidualScopeInstance::restore(
        &snapshot,
        prepared,
        checked.clone(),
        &log,
        ResidualStateLimits::default(),
    )
    .unwrap();
    assert_eq!(
        restored.snapshot(),
        original,
        "completion history and monitor state must be preserved"
    );
    assert!(matches!(
        restored
            .preview(
                ActionAttemptId::from_bytes(identity(b'a', 2)),
                SemanticEventCursor(3),
                &send
            )
            .unwrap(),
        ResidualPreview::Reject
    ));
    commit_approval(&mut restored, approval, 2, 3);
    assert!(matches!(
        restored
            .preview(
                ActionAttemptId::from_bytes(identity(b'a', 3)),
                SemanticEventCursor(5),
                &send
            )
            .unwrap(),
        ResidualPreview::Permit(_)
    ));
}

#[test]
fn pending_approval_cannot_restore_a_trusted_result_flag() {
    use crate::control::residual::{ResidualAttemptTag, ResidualObservedScope};
    let (checked, prepared, log, mut scope) = checked_approval_scope();
    let approval = label(&mut scope, &checked, "Approval.request");
    let send = label(&mut scope, &checked, "Console.stdout_write");
    let id = ActionAttemptId::from_bytes(identity(b'a', 1));
    let cursor = SemanticEventCursor(1);
    let ResidualPreview::Permit(deltas) = scope.preview(id, cursor, &approval).unwrap() else {
        panic!("permitted")
    };
    let version = scope.commit(deltas, id, cursor, approval.clone()).unwrap();
    let scope = Rc::new(std::cell::RefCell::new(scope));
    let tag = ResidualAttemptTag::new(
        id,
        cursor,
        vec![ResidualObservedScope {
            instance: scope.clone(),
            action: approval,
            version,
        }],
        true,
    );
    tag.consume_permit().unwrap();
    tag.record_trusted_approval(true);
    let saved = tag.snapshot(|_| Ok(0)).unwrap();
    let raw = serde_json::to_value(&saved).unwrap();
    assert!(raw.get("trusted_approval").is_none());
    assert!(raw.get("decision").is_none());
    let mut forged = raw;
    forged["trusted_approval"] = serde_json::json!(true);
    assert!(
        serde_json::from_value::<crate::orchestration::ResidualAttemptSnapshot>(forged).is_err()
    );
    let restored = Rc::new(std::cell::RefCell::new(
        ResidualScopeInstance::restore(
            &scope.borrow().snapshot(),
            prepared,
            checked,
            &log,
            ResidualStateLimits::default(),
        )
        .unwrap(),
    ));
    let restored_tag =
        ResidualAttemptTag::restore(&saved, std::slice::from_ref(&restored)).unwrap();
    let mut stale_permit = saved.clone();
    stale_permit.dispatched = false;
    let error =
        ResidualAttemptTag::restore(&stale_permit, std::slice::from_ref(&restored)).unwrap_err();
    assert!(error.contains("no safe authorization point"));
    restored_tag
        .complete(&value::InterpValue::Bool(true), SemanticEventCursor(2))
        .unwrap();
    assert!(matches!(
        restored
            .borrow()
            .preview(
                ActionAttemptId::from_bytes(identity(b'a', 2)),
                SemanticEventCursor(3),
                &send
            )
            .unwrap(),
        ResidualPreview::Reject
    ));
}

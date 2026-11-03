use super::*;
use crate::{
    api::{PlanOptions, RunOptions},
    eval::{EvalContext, EvalContextInput, EvalProgramInput},
    testing::project::checked_project,
};

fn program<'a>(
    checked: &'a crate::CheckedProject,
    plan: &'a crate::plan::InterpreterPlan,
) -> EvalProgramInput<'a> {
    EvalProgramInput {
        code_definitions: None,
        syntax_limits: Default::default(),
        checked,
        plan,
        entry_item: checked.entry.unwrap(),
        entry_args: &[],
    }
}

fn initialize<'a>(
    checked: &'a crate::CheckedProject,
    plan: &'a crate::plan::InterpreterPlan,
    options: RunOptions,
    consumed_steps: u64,
) -> (EvalProgramInput<'a>, EvalRunState) {
    EvalContextInput {
        replay_keys: crate::eval::replay::RunReplayKeys::start(&options).unwrap(),
        code_definitions: options.code_definitions,
        syntax_limits: options.syntax_limits,
        checked,
        plan,
        entry_item: checked.entry.unwrap(),
        entry_args: &[],
        storage_limits: options.storage_limits,
        event_observer: options.event_observer,
        execution: etas_host::execution::ExecutionScope::new_owned(),
        host_context: options.host_context,
        model_policy: options.model_policy,
        execution_limits: options.execution_limits,
        consumed_steps,
        current_session: options.current_session,
    }
    .into_parts()
}

#[test]
fn executable_code_ownership_shares_one_plan_instead_of_copying_it_per_call() {
    let checked = checked_project("module app.main; flow main() -> i32 { return 1; }");
    let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
    let (input, mut state) = initialize(&checked, &plan, RunOptions::default(), 0);
    let mut ctx = EvalContext::new(input, &mut state);
    let first = ctx.checked_execution_program();
    let second = ctx.checked_execution_program();
    assert!(std::sync::Arc::ptr_eq(&first.plan, &second.plan));
    let scoped = ctx.scoped_context(&first, 0);
    let third = scoped.checked_execution_program();
    assert!(std::sync::Arc::ptr_eq(&first.plan, &third.plan));
}

#[test]
fn program_scopes_share_fuel_grants_digest_budget_and_cancellation() {
    let original = checked_project("module app.main; flow main() -> i32 { return 1; }");
    let candidate =
        checked_project("module app.main; flow main() -> string { return \"candidate\"; }");
    let original_plan = crate::Interpreter
        .plan(&original, PlanOptions)
        .plan
        .unwrap();
    let candidate_plan = crate::Interpreter
        .plan(&candidate, PlanOptions)
        .plan
        .unwrap();
    let options = RunOptions {
        execution_limits: ExecutionLimits {
            max_steps: std::num::NonZeroU64::new(42),
            ..Default::default()
        },
        ..Default::default()
    };
    let (input, mut state) = initialize(&original, &original_plan, options, 40);
    let scope = state.execution.clone();
    let trace = state.host_context.trace.clone();
    let digest = state.host_trace_digest_key.clone().unwrap();
    let replay_key_id = state.replay_keys.id();
    {
        let mut ctx = EvalContext::new(input, &mut state);
        let span = crate::diagnostics::item_span(ctx.checked, ctx.entry_item);
        ctx.consume_execution_step(span).unwrap();
        assert_eq!(ctx.next_host_request_id(), HostRequestId(0));
        ctx.host_context
            .authority
            .grants
            .push(etas_host::HostActionGrant::allow("Code", "check"));
        ctx.current_session = Some("shared-session".into());
        let reservation = ctx.host_context.budget.reserve_tokens(7).unwrap();
        ctx.host_context
            .budget
            .settle_tokens(reservation, 7)
            .unwrap();
    }
    {
        let mut ctx = EvalContext::new(program(&candidate, &candidate_plan), &mut state);
        let span = crate::diagnostics::item_span(ctx.checked, ctx.entry_item);
        assert_eq!(ctx.safe_points.consumed_steps(), 41);
        assert_eq!(ctx.next_host_request_id(), HostRequestId(1));
        assert!(
            ctx.host_context
                .authority
                .grants
                .contains(&etas_host::HostActionGrant::allow("Code", "check"))
        );
        assert_eq!(ctx.current_session.as_deref(), Some("shared-session"));
        assert_eq!(
            ctx.host_context.budget.snapshot().unwrap().consumed_tokens,
            7
        );
        assert_eq!(ctx.host_context.trace, trace);
        assert_eq!(ctx.host_trace_digest_key().unwrap(), &digest);
        assert_eq!(ctx.replay_keys.id(), replay_key_id);
        ctx.consume_execution_step(span).unwrap();
        assert!(
            ctx.consume_execution_step(span)
                .unwrap_err()
                .message
                .contains("execution steps (42)")
        );
        scope
            .cancel_source()
            .stop(etas_host::execution::CancellationReason::Requested)
            .unwrap();
        assert!(matches!(
            ctx.cancellation_signal(span),
            Some(crate::control::ControlSignal::Cancelled(_))
        ));
    }
    assert_eq!(state.safe_points.consumed_steps(), 42);
}

#[tokio::test(flavor = "current_thread")]
async fn checked_program_scopes_execute_without_reinitializing_the_invocation() {
    let original = checked_project("module app.main; flow main() -> i32 { return 1; }");
    let candidate =
        checked_project("module app.main; flow main() -> string { return \"candidate\"; }");
    let original_plan = crate::Interpreter
        .plan(&original, PlanOptions)
        .plan
        .unwrap();
    let candidate_plan = crate::Interpreter
        .plan(&candidate, PlanOptions)
        .plan
        .unwrap();
    let (input, mut state) = initialize(&original, &original_plan, RunOptions::default(), 30);
    let host = crate::testing::FakeHost::new(Default::default());
    let first_steps = {
        let mut ctx = EvalContext::new(input, &mut state);
        let signal = ctx.execute_entry_signal(ctx.entry_item);
        let outcome = crate::driver::execute_entry(&mut ctx, signal, &host)
            .await
            .into_outcome();
        assert!(
            matches!(outcome, crate::api::RunOutcome::Completed(value) if value == crate::api::InterpValue::i32(1))
        );
        ctx.safe_points.consumed_steps()
    };
    {
        let mut ctx = EvalContext::new(program(&candidate, &candidate_plan), &mut state);
        assert_eq!(ctx.safe_points.consumed_steps(), first_steps);
        let signal = ctx.execute_entry_signal(ctx.entry_item);
        let outcome = crate::driver::execute_entry(&mut ctx, signal, &host)
            .await
            .into_outcome();
        assert!(
            matches!(outcome, crate::api::RunOutcome::Completed(crate::api::InterpValue::String(value)) if value.as_str() == "candidate")
        );
        assert!(ctx.safe_points.consumed_steps() > first_steps);
    }
    assert!(state.diagnostics.is_empty());
    let events = state.events.into_events();
    let started: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            crate::orchestration::WorkflowEvent::StepStarted(id) => Some(id.0),
            _ => None,
        })
        .collect();
    assert_eq!(started, [0, 2]);
}

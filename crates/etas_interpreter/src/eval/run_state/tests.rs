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
        cpu_threads: options.cpu_threads,
        runtime: options.runtime.clone(),
        cpu_min_work: options.cpu_min_work,
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
fn sibling_requests_keep_approvals_local_and_share_the_existing_budget() {
    use crate::driver::scheduler::{TaskId, TaskScopeId};
    use etas_host::{ActionInstance, ApprovalGrant, Budget, HostActionGrant, TokenBudget};

    let checked = checked_project("module app.main; flow main() -> i32 { return 1; }");
    let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
    let mut options = RunOptions::default();
    options.host_context.budget = etas_host::ExecutionBudget::start(Budget {
        tokens: Some(TokenBudget { max_tokens: 10 }),
        ..Default::default()
    });
    let (input, mut state) = initialize(&checked, &plan, options, 0);
    let mut ctx = EvalContext::new(input, &mut state);
    let program = ctx.checked_execution_program();
    let mut first = ctx.task.child_in_scope(TaskScopeId(0), TaskId(1)).unwrap();
    let mut second = ctx.task.child_in_scope(TaskScopeId(0), TaskId(2)).unwrap();
    let action = ActionInstance::new("Console", "stdout_write", Vec::new());
    {
        let mut request = ctx.scheduled_task_context(&program, &mut first, TaskId(1), 0);
        let id = request.next_host_request_id();
        request
            .record_approval_grant(ApprovalGrant {
                id,
                grants: vec![HostActionGrant::allow("Console", "stdout_write")],
            })
            .unwrap();
        assert!(request.host_authority().allows(&action));
        let budget = request.host_budget();
        let reservation = budget.reserve_tokens(7).unwrap();
        budget.settle_tokens(reservation, 7).unwrap();
    }
    {
        let request = ctx.scheduled_task_context(&program, &mut second, TaskId(2), 0);
        assert!(!request.host_authority().allows(&action));
        assert_eq!(request.host_budget().remaining_tokens().unwrap(), Some(3));
    }
    assert!(!ctx.host_authority().allows(&action));
    assert_eq!(ctx.host_budget().snapshot().unwrap().consumed_tokens, 7);
    let mut descendant = first.child_in_scope(TaskScopeId(0), TaskId(3)).unwrap();
    let request = ctx.scheduled_task_context(&program, &mut descendant, TaskId(3), 0);
    assert!(request.host_authority().allows(&action));
    assert_eq!(request.host_budget().remaining_tokens().unwrap(), Some(3));
}

#[test]
fn a_handler_host_loan_keeps_request_accounts_approvals_and_receiver_constraints() {
    use crate::driver::scheduler::{TaskId, TaskScopeId};
    use etas_host::{
        ActionInstance, ApprovalGrant, AuthorityContext, Budget, HostActionGrant, TokenBudget,
    };

    let checked = checked_project("module app.main; flow main() -> i32 { return 0; }");
    let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
    let mut options = RunOptions::default();
    options.host_context.authority.grants = vec![HostActionGrant::allow("Console", "stdout_write")];
    let (input, mut state) = initialize(&checked, &plan, options, 0);
    let mut ctx = EvalContext::new(input, &mut state);
    let program = ctx.checked_execution_program();
    let owner = etas_host::ExecutionBudget::start(Budget {
        tokens: Some(TokenBudget { max_tokens: 100 }),
        ..Default::default()
    });
    let caller = etas_host::ExecutionBudget::start(Budget {
        tokens: Some(TokenBudget { max_tokens: 10 }),
        ..Default::default()
    });
    let mut request = ctx.task.child_in_scope(TaskScopeId(0), TaskId(1)).unwrap();
    request.agent_request_root = Some(request.identity);
    request.host_context.budget = caller.conjoin(&owner).unwrap();
    let mut restriction = AuthorityContext::deny_all();
    restriction.grants = vec![HostActionGrant::allow("Clock", "now")];
    request.authority_constraints.push(restriction);
    let loan = ctx
        .task
        .borrow_agent_host_context(request.capture_agent_host_context().unwrap(), None);
    let id = ctx.next_host_request_id();
    ctx.record_approval_grant(ApprovalGrant {
        id,
        grants: vec![HostActionGrant::allow("Console", "stdout_write")],
    })
    .unwrap();
    let action = ActionInstance::new("Console", "stdout_write", Vec::new());
    assert!(!ctx.host_authority().allows(&action));
    let budget = ctx.host_budget();
    let descendant = ctx.task.child_in_scope(TaskScopeId(0), TaskId(4)).unwrap();
    assert_eq!(descendant.parent, Some(request.identity));
    let tokens = budget.reserve_tokens(7).unwrap();
    budget.settle_tokens(tokens, 7).unwrap();
    let completion = crate::eval::host_completion::HostCompletionContext::capture(&ctx);
    drop(loan);
    let mut second = ctx.task.child_in_scope(TaskScopeId(0), TaskId(2)).unwrap();
    second.agent_request_root = Some(second.identity);
    let second_loan = ctx
        .task
        .borrow_agent_host_context(second.capture_agent_host_context().unwrap(), None);
    let later = ctx.next_host_request_id();
    completion.run(&mut ctx, |ctx| {
        assert_eq!(ctx.host_budget().remaining_tokens().unwrap(), Some(3));
        ctx.record_approval_grant(ApprovalGrant {
            id: later,
            grants: vec![],
        })
        .unwrap();
    });
    assert!(ctx.host_authority().approvals.is_empty());
    drop(second_loan);
    request
        .execution
        .cancel_source()
        .stop(etas_host::execution::CancellationReason::Requested)
        .unwrap();
    assert!(descendant.cancellation.as_ref().unwrap().is_cancelled());
    assert!(!ctx.task.cancellation.as_ref().unwrap().is_cancelled());
    assert!(ctx.host_authority().approvals.is_empty());
    assert!(ctx.host_authority().allows(&action));
    assert_eq!(owner.snapshot().unwrap().consumed_tokens, 7);
    assert_eq!(caller.snapshot().unwrap().consumed_tokens, 7);
    let request = ctx.scheduled_task_context(&program, &mut request, TaskId(1), 0);
    assert_eq!(request.host_authority().approvals[0].id, id);
    assert_eq!(request.host_authority().approvals[1].id, later);
    assert_eq!(request.host_budget().remaining_tokens().unwrap(), Some(3));
}

#[test]
fn agent_admission_conjoins_caller_and_owner_accounts_and_authority() {
    use etas_host::{ActionInstance, Budget, HostActionGrant, TokenBudget};

    let checked =
        checked_project("module app.main; agent Service = {}; flow main() -> i32 { return 0; }");
    let ty = checked
        .types
        .symbol_types
        .values()
        .find_map(|fact| match fact {
            etas_types::SymbolTypeFact::Agent { signature } => Some(signature.component),
            _ => None,
        })
        .unwrap();
    let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
    let mut options = RunOptions::default();
    let owner = etas_host::ExecutionBudget::start(Budget {
        tokens: Some(TokenBudget { max_tokens: 100 }),
        ..Default::default()
    });
    let tokens = owner.reserve_tokens(80).unwrap();
    owner.settle_tokens(tokens, 80).unwrap();
    options.host_context.budget = owner.clone();
    options.host_context.authority.grants = vec![HostActionGrant::allow("Clock", "now")];
    let (input, mut state) = initialize(&checked, &plan, options, 0);
    let mut ctx = EvalContext::new(input, &mut state);
    ctx.task.model_policy.model_locked = true;
    ctx.task.model_policy.model = etas_host::ModelName("receiver-model".into());
    ctx.task.model_policy.options.max_output_tokens = Some(6);
    ctx.task.model_policy.max_tool_rounds = 2;
    ctx.task.model_policy.budget = Some(Budget {
        tokens: Some(TokenBudget { max_tokens: 88 }),
        ..Default::default()
    });
    let program = ctx.checked_execution_program();
    let reference = ctx
        .scheduler
        .construct_agent(
            ctx.task,
            program.clone(),
            ty,
            crate::value::InterpValue::Record(Vec::new().into()),
        )
        .unwrap();
    let caller = etas_host::ExecutionBudget::start(Budget {
        tokens: Some(TokenBudget { max_tokens: 10 }),
        ..Default::default()
    });
    ctx.task.host_context.budget = caller.clone();
    ctx.task.model_policy = crate::api::ModelExecutionPolicy::default();
    ctx.task
        .host_context
        .authority
        .grants
        .push(HostActionGrant::allow("Console", "stdout_write"));
    let handle = ctx
        .scheduler
        .spawn(
            ctx.task,
            crate::eval::machine::EvalMachine::new(),
            Some(program.clone()),
            0,
            0,
        )
        .unwrap();
    ctx.scheduler
        .admit_agent_request(handle, reference, false, &program)
        .unwrap();
    assert_eq!(ctx.scheduler.dispatch_ready(), Some(handle));
    let mut task = ctx.scheduler.take_running(handle).unwrap();
    {
        let request = ctx.scheduled_task_context(&program, &mut task.local, handle.task, 0);
        assert!(
            request
                .host_authority()
                .allows(&ActionInstance::new("Clock", "now", vec![]))
        );
        assert!(!request.host_authority().allows(&ActionInstance::new(
            "Console",
            "stdout_write",
            vec![]
        )));
        let budget = request.host_budget();
        let mut policy = request.task.model_policy.clone();
        policy.options.max_output_tokens = Some(32);
        request
            .constrain_model_policy(&mut policy, request.limit_span)
            .unwrap();
        assert_eq!(policy.model.0, "receiver-model");
        assert!(policy.model_locked);
        assert_eq!(policy.options.max_output_tokens, Some(6));
        assert_eq!(policy.max_tool_rounds, 2);
        assert_eq!(budget.remaining_tokens().unwrap(), Some(8));
        let tokens = budget.reserve_tokens(7).unwrap();
        budget.settle_tokens(tokens, 7).unwrap();
    }
    assert_eq!(owner.snapshot().unwrap().consumed_tokens, 87);
    assert_eq!(caller.snapshot().unwrap().consumed_tokens, 7);
    assert!(
        ctx.host_authority()
            .allows(&ActionInstance::new("Console", "stdout_write", vec![]))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_owner_policy_is_checked_even_when_the_caller_has_no_policy() {
    use etas_host::{AuthorityContext, HostActionGrant, HostValue};

    let checked = checked_project(
        "module app.main; import std.effects.Console; import std.io.println; \
         flow main() -> unit ![Console, Error<IOError>] { println(\"hi\"); }",
    );
    let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
    let mut options = RunOptions::default();
    options.host_context.authority.grants = vec![HostActionGrant::allow("Console", "stdout_write")];
    let (input, mut state) = initialize(&checked, &plan, options, 0);
    let mut ctx = EvalContext::new(input, &mut state);
    let policy_ref = HostValue::String("owner-policy".into());
    let mut owner = AuthorityContext::deny_all();
    owner.grants = vec![HostActionGrant::allow("Console", "stdout_write")];
    owner.policy.boundary_policy_ref = Some(policy_ref.clone());
    ctx.task.authority_constraints.push(owner);
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
        console: true,
        policy: true,
        host_authority: true,
        ..Default::default()
    });
    host.deny_policy("owner restriction");
    let signal = ctx.execute_entry_signal(ctx.entry_item);
    let _ = crate::driver::execute_entry(&mut ctx, signal, &host).await;
    let requests = host.policy_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].policy_ref, policy_ref);
    assert_eq!(host.console_call_count(), 0);
    assert!(
        ctx.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("owner restriction"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_inherited_receiver_model_policy_is_retained_through_agent_dispatch() {
    use etas_host::{HostActionGrant, HostValue};
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
agent Writer = {};
impl Writer {
    public flow answer(self) -> string {
        return perform infer(Prompt.new().user(Public("hello")));
    }
}
flow main() -> string {
    let writer = Writer {};
    return writer.answer();
}
"#,
    );
    let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
    let mut options = RunOptions::default();
    options.host_context.authority.grants = vec![
        HostActionGrant::allow("Agentic", "infer"),
        HostActionGrant::allow("Agentic", "step"),
    ];
    options.model_policy.provider_capabilities = Some(crate::testing::full_model_capabilities());
    options.model_policy.max_tool_rounds = 0;
    let caller_ref = HostValue::String("caller-model-policy".into());
    let receiver_ref = HostValue::String("receiver-model-policy".into());
    options.model_policy.policy_ref = Some(caller_ref.clone());
    let (input, mut state) = initialize(&checked, &plan, options, 0);
    let mut ctx = EvalContext::new(input, &mut state);
    let mut receiver = ctx.task.model_policy.clone();
    receiver.policy_ref = Some(receiver_ref.clone());
    ctx.task.model_restrictions.push(
        super::super::model_restrictions::AgentModelRestrictions::capture(
            &receiver,
            Default::default(),
        ),
    );
    let mut availability = crate::host::HostServiceAvailability::default();
    availability.enable(etas_effects::HostRequirementKind::Agentic);
    availability.policy = true;
    availability.host_authority = true;
    let host = crate::testing::FakeHost::new(availability);
    host.seed_model_response_text("draft");
    let signal = ctx.execute_entry_signal(ctx.entry_item);
    let outcome = crate::driver::execute_entry(&mut ctx, signal, &host).await;
    assert!(ctx.diagnostics.is_empty(), "{:?}", ctx.diagnostics);
    assert!(
        matches!(outcome.into_outcome(), crate::api::RunOutcome::Completed(value) if value == crate::value::InterpValue::String("draft".into()))
    );
    let requests = host.policy_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].policy_ref, caller_ref);
    assert_eq!(requests[1].policy_ref, receiver_ref);
    assert!(
        requests
            .iter()
            .all(|request| request.subject.kind == "model")
    );
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
    let scope = state.root_task.execution.clone();
    let trace = state.root_task.host_context.trace.clone();
    let digest = state.host_trace_digest_key.clone().unwrap();
    let replay_key_id = state.replay_keys.id();
    {
        let mut ctx = EvalContext::new(input, &mut state);
        let span = crate::diagnostics::item_span(ctx.checked, ctx.entry_item);
        ctx.consume_execution_step(span).unwrap();
        assert_eq!(ctx.next_host_request_id(), HostRequestId(0));
        ctx.task
            .host_context
            .authority
            .grants
            .push(etas_host::HostActionGrant::allow("Code", "check"));
        ctx.current_session = Some("shared-session".into());
        let reservation = ctx.task.host_context.budget.reserve_tokens(7).unwrap();
        ctx.task
            .host_context
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
            ctx.task
                .host_context
                .authority
                .grants
                .contains(&etas_host::HostActionGrant::allow("Code", "check"))
        );
        assert_eq!(ctx.current_session.as_deref(), Some("shared-session"));
        assert_eq!(
            ctx.task
                .host_context
                .budget
                .snapshot()
                .unwrap()
                .consumed_tokens,
            7
        );
        assert_eq!(ctx.task.host_context.trace, trace);
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
    assert!(state.shared.diagnostics.is_empty());
    let events = state.shared.events.into_events();
    let started: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            crate::orchestration::WorkflowEvent::StepStarted(id) => Some(id.0),
            _ => None,
        })
        .collect();
    assert_eq!(started, [0, 2]);
}

#[test]
fn unfinished_agent_construction_owns_fresh_children_before_publication() {
    let checked =
        checked_project("module app.main; agent Service = {}; flow main() -> i32 { return 0; }");
    let ty = checked
        .types
        .symbol_types
        .values()
        .find_map(|fact| match fact {
            etas_types::SymbolTypeFact::Agent { signature } => Some(signature.component),
            _ => None,
        })
        .unwrap();
    let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
    let (input, mut state) = initialize(&checked, &plan, RunOptions::default(), 0);
    let ctx = EvalContext::new(input, &mut state);
    let program = ctx.checked_execution_program();
    let borrowed = ctx
        .scheduler
        .begin_agent_construction(ctx.task, program.clone(), ty)
        .unwrap();
    ctx.scheduler
        .publish_agent_construction(
            &borrowed,
            crate::value::InterpValue::Record(Vec::new().into()),
        )
        .unwrap();
    let outer = ctx
        .scheduler
        .begin_agent_construction(ctx.task, program.clone(), ty)
        .unwrap();
    ctx.task
        .agent_initializations
        .push(std::rc::Rc::downgrade(&outer));
    assert_eq!(
        ctx.task.host_execution().id(),
        outer.lifetime.execution.id()
    );
    let child = ctx
        .scheduler
        .begin_agent_construction(ctx.task, program, ty)
        .unwrap();
    assert_ne!(outer.reference, child.reference);
    ctx.scheduler
        .publish_agent_construction(&child, crate::value::InterpValue::Record(Vec::new().into()))
        .unwrap();
    let outer_scope = outer.lifetime.execution.clone();
    let child_scope = child.lifetime.execution.clone();
    drop(child);
    drop(outer);
    assert!(outer_scope.termination().unwrap().is_some());
    assert!(child_scope.termination().unwrap().is_some());
    assert!(borrowed.lifetime.execution.termination().unwrap().is_none());
    assert!(ctx.task.pending_agent_initialization().is_none());
    ctx.scheduler.reap_closed_agents().unwrap();
    ctx.scheduler
        .close_agent_scope(crate::driver::scheduler::TaskScopeId(0))
        .unwrap();
    assert!(borrowed.lifetime.execution.termination().unwrap().is_some());
}

#[test]
fn record_constructor_publishes_fresh_ownership_and_only_aborts_new_children() {
    let checked =
        checked_project("module app.main; agent Service = {}; flow main() -> i32 { return 0; }");
    let ty = checked
        .types
        .symbol_types
        .values()
        .find_map(|fact| match fact {
            etas_types::SymbolTypeFact::Agent { signature } => Some(signature.component),
            _ => None,
        })
        .unwrap();
    let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
    let (input, mut state) = initialize(&checked, &plan, RunOptions::default(), 0);
    let ctx = EvalContext::new(input, &mut state);
    let program = ctx.checked_execution_program();
    let borrowed = ctx
        .scheduler
        .begin_agent_construction(ctx.task, program.clone(), ty)
        .unwrap();
    ctx.scheduler
        .publish_agent_construction(
            &borrowed,
            crate::value::InterpValue::Record(Vec::new().into()),
        )
        .unwrap();
    let success = ctx.scheduler.begin_record_construction(ctx.task);
    ctx.task.register_record_construction(&success);
    let first = ctx
        .scheduler
        .begin_agent_construction(ctx.task, program.clone(), ty)
        .unwrap();
    ctx.scheduler
        .publish_agent_construction(&first, crate::value::InterpValue::Record(Vec::new().into()))
        .unwrap();
    success.publish();
    drop(success);
    assert!(first.lifetime.execution.termination().unwrap().is_none());
    let interrupted = ctx.scheduler.begin_record_construction(ctx.task);
    ctx.task.register_record_construction(&interrupted);
    let second = ctx
        .scheduler
        .begin_agent_construction(ctx.task, program, ty)
        .unwrap();
    ctx.scheduler
        .publish_agent_construction(
            &second,
            crate::value::InterpValue::Record(Vec::new().into()),
        )
        .unwrap();
    drop(interrupted);
    assert!(second.lifetime.execution.termination().unwrap().is_some());
    assert!(first.lifetime.execution.termination().unwrap().is_none());
    assert!(borrowed.lifetime.execution.termination().unwrap().is_none());
    ctx.scheduler
        .close_agent_scope(crate::driver::scheduler::TaskScopeId(0))
        .unwrap();
}

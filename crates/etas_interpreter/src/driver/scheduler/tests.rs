use std::num::NonZeroUsize;

use super::*;
use crate::{
    api::ModelExecutionPolicy,
    eval::{TaskLocalState, machine::EvalMachine},
};

pub(super) fn root_local(scope: TaskScopeId) -> TaskLocalState {
    TaskLocalState::root(
        scope,
        etas_host::execution::ExecutionScope::new_owned(),
        ModelExecutionPolicy::default(),
        Default::default(),
    )
}

pub(super) fn spawn(
    scheduler: &mut TaskScheduler,
    parent: &TaskLocalState,
    source_index: usize,
) -> TaskHandle {
    scheduler
        .spawn(parent, EvalMachine::new(), None, 0, source_index)
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn task_completion_waits_for_owned_resource_release_without_holding_a_language_slot() {
    let scope = TaskScopeId(7);
    let root = root_local(scope);
    let mut scheduler = TaskScheduler::new(scope, NonZeroUsize::new(1).unwrap());
    let owner = spawn(&mut scheduler, &root, 0);
    let sibling = spawn(&mut scheduler, &root, 1);
    scheduler
        .set_scope_owner(scope, owner, TaskIdentity::Root)
        .unwrap();
    let resource = scheduler.tasks[&owner.task]
        .local
        .execution
        .register(
            None,
            None,
            etas_host::TraceContext::root(etas_host::TraceId(1)),
        )
        .unwrap();
    resource.begin_dispatch().unwrap();
    assert_eq!(scheduler.dispatch_ready(), Some(owner));
    scheduler.complete(owner, InterpValue::Unit).unwrap();
    assert_eq!(
        scheduler.tasks[&owner.task].state,
        TaskState::Waiting(TaskWait::HostDrain)
    );
    assert!(scheduler.tasks[&owner.task].outcome.is_none());
    assert_eq!(scheduler.dispatch_ready(), Some(sibling));
    scheduler.complete(sibling, InterpValue::Unit).unwrap();

    let mut reactor = TaskHostReactor::new();
    for (task, execution) in scheduler.take_resource_drains() {
        reactor.drain_resources(task, execution);
    }
    assert!(reactor.poll_ready().await.is_none());
    resource
        .complete(etas_host::execution::ExternalOutcome::Confirmed, vec![])
        .unwrap();
    let ReadyTaskHost::Cleanup { task, report } = reactor.wait_next().await.unwrap() else {
        panic!("resource owner completion");
    };
    assert_eq!(task, owner);
    scheduler
        .finish_resource_drain(task, report.unwrap())
        .unwrap();
    assert!(matches!(
        scheduler.tasks[&owner.task].outcome,
        Some(TaskOutcome::Completed(InterpValue::Unit))
    ));
    let report = scheduler.tasks[&owner.task]
        .cleanup_report
        .as_ref()
        .unwrap();
    assert_eq!(report.operations().len(), 1);
    assert!(!reactor.has_pending());
}

#[test]
fn fifo_admission_and_waiting_release_concurrency_slots() {
    let scope = TaskScopeId(7);
    let root = root_local(scope);
    let mut scheduler = TaskScheduler::new(scope, NonZeroUsize::new(1).unwrap());
    let first = spawn(&mut scheduler, &root, 0);
    let second = spawn(&mut scheduler, &root, 1);

    assert_eq!(scheduler.dispatch_ready(), Some(first));
    assert_eq!(scheduler.dispatch_ready(), None);
    scheduler.requeue(first).unwrap();
    assert_eq!(scheduler.dispatch_ready(), Some(second));

    assert!(matches!(
        scheduler.await_task(second, first).unwrap(),
        AwaitDecision::Parked
    ));
    assert_eq!(scheduler.dispatch_ready(), Some(first));
}

#[tokio::test(flavor = "current_thread")]
async fn resource_cleanup_diagnostics_are_returned_as_checked_source_data() {
    let checked = crate::testing::project::checked_project(
        r#"
module app.main;
type CleanupFailure = { resource_kind: string, code: string, message: string };
type ScopeReport<T> = { value: T, failures: Array<CleanupFailure> };
flow main() -> ScopeReport<unit> { return ScopeReport { value= (), failures= [] }; }
"#,
    );
    let etas_types::ItemSignature::Flow(signature) =
        &checked.types.item_signatures[&checked.entry.unwrap()]
    else {
        unreachable!()
    };
    let report_type = signature.output;
    let plan = crate::plan::build_plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let scope = TaskScopeId(7);
    let root = root_local(scope);
    let mut scheduler = TaskScheduler::new(scope, NonZeroUsize::new(1).unwrap());
    let owner = spawn(&mut scheduler, &root, 0);
    scheduler.tasks.get_mut(&owner.task).unwrap().program =
        Some(crate::value::CheckedExecutionProgram {
            checked,
            plan: std::sync::Arc::new(plan),
            definitions: None,
            revision: None,
            foreign: None,
        });
    scheduler
        .set_scope_owner(scope, owner, TaskIdentity::Root)
        .unwrap();
    let span = etas_core::Span::empty(etas_core::SourceId(0), etas_core::TextSize::ZERO);
    scheduler.configure_resource_scope(scope, report_type, span);
    let resource = scheduler.tasks[&owner.task]
        .local
        .execution
        .register(
            None,
            None,
            etas_host::TraceContext::root(etas_host::TraceId(1)),
        )
        .unwrap();
    resource.begin_dispatch().unwrap();
    assert_eq!(scheduler.dispatch_ready(), Some(owner));
    scheduler.complete(owner, InterpValue::Unit).unwrap();
    let mut reactor = TaskHostReactor::new();
    for (task, execution) in scheduler.take_resource_drains() {
        reactor.drain_resources(task, execution);
    }
    resource
        .complete(
            etas_host::execution::ExternalOutcome::Confirmed,
            vec![etas_host::HostError::new(
                etas_host::HostErrorCode::ProviderUnavailable,
                "private backend detail",
            )],
        )
        .unwrap();
    let ReadyTaskHost::Cleanup { task, report } = reactor.wait_next().await.unwrap() else {
        panic!("cleanup completion");
    };
    scheduler
        .finish_resource_drain(task, report.unwrap())
        .unwrap();
    let Some(TaskOutcome::Completed(value)) = scheduler.tasks[&owner.task].outcome.as_ref() else {
        panic!("source cleanup report");
    };
    let payload = crate::eval::interp_to_host_value(value).unwrap();
    assert!(
        matches!(payload, etas_host::HostValue::Record(ref fields) if fields.iter().any(|(name, value)| name == "failures" && matches!(value, etas_host::HostValue::List(values) if values.len() == 1)))
    );
    assert!(!format!("{payload:?}").contains("private backend detail"));
    // Native completion returns data; the public Etas scope flow owns the
    // CleanupFailed construction and raise. Existing control stays primary.
    let report = scheduler.tasks[&owner.task]
        .cleanup_report
        .as_ref()
        .unwrap();
    let program = scheduler.tasks[&owner.task].program.as_ref().unwrap();
    let string = program
        .checked
        .type_store
        .iter()
        .find_map(|(ty, value)| {
            matches!(
                value,
                etas_types::Type::Primitive(etas_types::PrimitiveType::String)
            )
            .then_some(ty)
        })
        .unwrap();
    let primary = TaskOutcome::Raised(Box::new(crate::control::PendingPerform::typed_error(
        string,
        InterpValue::String("primary".into()),
        span,
        crate::control::Continuation::BlockValue,
    )));
    let preserved =
        resource_scope::completion((report_type, span), report, program, primary).unwrap();
    assert!(
        matches!(preserved, TaskOutcome::Raised(error) if error.error_type == Some(string) && error.args == [InterpValue::String("primary".into())])
    );

    // The body error also stays primary while child work is unwinding, even
    // when that child's declaration precedes the body in source order.
    let program = program.clone();
    let scope = TaskScopeId(8);
    let root = root_local(scope);
    let mut scheduler = TaskScheduler::new(scope, NonZeroUsize::new(2).unwrap());
    let owner = spawn(&mut scheduler, &root, 10);
    scheduler.tasks.get_mut(&owner.task).unwrap().program = Some(program.clone());
    scheduler
        .set_scope_owner(scope, owner, TaskIdentity::Root)
        .unwrap();
    scheduler.configure_resource_scope(scope, report_type, span);
    let body = scheduler.tasks.remove(&owner.task).unwrap();
    let child = spawn(&mut scheduler, &body.local, 0);
    scheduler.tasks.insert(owner.task, body);
    assert_eq!(scheduler.dispatch_ready(), Some(owner));
    assert_eq!(scheduler.dispatch_ready(), Some(child));
    let error = |message: &str| {
        crate::control::PendingPerform::typed_error(
            string,
            InterpValue::String(message.into()),
            span,
            crate::control::Continuation::BlockValue,
        )
    };
    scheduler.raise(owner, error("primary")).unwrap();
    scheduler
        .cancel(owner, etas_host::execution::CancellationReason::ChildFailed)
        .unwrap();
    scheduler.raise(child, error("secondary")).unwrap();
    assert!(matches!(
        scheduler.scope_outcome(scope).unwrap(),
        Some(TaskOutcome::Raised(error)) if error.args == [InterpValue::String("primary".into())]
    ));

    let scope = TaskScopeId(9);
    let root = root_local(scope);
    let mut scheduler = TaskScheduler::new(scope, NonZeroUsize::new(2).unwrap());
    let owner = spawn(&mut scheduler, &root, 0);
    scheduler.tasks.get_mut(&owner.task).unwrap().program = Some(program);
    scheduler
        .set_scope_owner(scope, owner, TaskIdentity::Root)
        .unwrap();
    scheduler.configure_resource_scope(scope, report_type, span);
    let body = scheduler.tasks.remove(&owner.task).unwrap();
    let child = spawn(&mut scheduler, &body.local, 1);
    scheduler.tasks.insert(owner.task, body);
    assert_eq!(scheduler.dispatch_ready(), Some(owner));
    assert_eq!(scheduler.dispatch_ready(), Some(child));
    scheduler.complete(owner, InterpValue::Unit).unwrap();
    let reason = etas_host::execution::CancellationReason::Requested;
    scheduler.cancel(owner, reason.clone()).unwrap();
    scheduler.cancel(child, reason).unwrap();
    assert!(matches!(
        scheduler.scope_outcome(scope).unwrap(),
        Some(TaskOutcome::Cancelled(_))
    ));
}

#[test]
fn host_wait_releases_its_slot_and_wakes_at_the_fifo_tail() {
    let scope = TaskScopeId(8);
    let root = root_local(scope);
    let operation = root
        .execution
        .register(
            None,
            None,
            etas_host::TraceContext::root(etas_host::TraceId(1)),
        )
        .unwrap();
    let operation_id = operation.context().operation();
    let mut scheduler = TaskScheduler::new(scope, NonZeroUsize::new(1).unwrap());
    let first = spawn(&mut scheduler, &root, 0);
    let second = spawn(&mut scheduler, &root, 1);

    assert_eq!(scheduler.dispatch_ready(), Some(first));
    scheduler.wait_for_host(first, operation_id).unwrap();
    assert_eq!(scheduler.dispatch_ready(), Some(second));
    scheduler.requeue(second).unwrap();

    scheduler.wake_from_host(first, operation_id).unwrap();
    assert_eq!(scheduler.dispatch_ready(), Some(second));
    scheduler.requeue(second).unwrap();
    assert_eq!(scheduler.dispatch_ready(), Some(first));
    operation
        .complete(
            etas_host::execution::ExternalOutcome::NotDispatched,
            Vec::new(),
        )
        .unwrap();
}

#[test]
fn checkpoint_barrier_keeps_fifo_and_waiters_while_committing_host_completions() {
    let scope = TaskScopeId(8);
    let root = root_local(scope);
    let operation = root
        .execution
        .register(
            None,
            None,
            etas_host::TraceContext::root(etas_host::TraceId(1)),
        )
        .unwrap();
    let operation_id = operation.context().operation();
    let mut scheduler = TaskScheduler::new(scope, NonZeroUsize::new(2).unwrap());
    let host = spawn(&mut scheduler, &root, 0);
    let waiter = spawn(&mut scheduler, &root, 1);
    let ready = spawn(&mut scheduler, &root, 2);
    assert_eq!(scheduler.dispatch_ready(), Some(host));
    scheduler.wait_for_host(host, operation_id).unwrap();
    assert_eq!(scheduler.dispatch_ready(), Some(waiter));
    assert!(matches!(
        scheduler.await_task(waiter, host).unwrap(),
        AwaitDecision::Parked
    ));

    scheduler.begin_checkpoint_barrier();
    assert_eq!(scheduler.dispatch_ready(), None);
    scheduler.wake_from_host(host, operation_id).unwrap();
    let task = scheduler.take_checkpoint_task(host);
    scheduler.restore_checkpoint_task(host, task);
    operation.begin_dispatch().unwrap();
    operation
        .complete(etas_host::execution::ExternalOutcome::Confirmed, Vec::new())
        .unwrap();
    assert_eq!(scheduler.dispatch_ready(), None);
    scheduler.complete(host, InterpValue::i32(42)).unwrap();
    assert_eq!(scheduler.state(waiter).unwrap(), TaskState::Ready);
    assert_eq!(scheduler.dispatch_ready(), None);
    scheduler.end_checkpoint_barrier();

    assert_eq!(scheduler.dispatch_ready(), Some(ready));
    assert_eq!(scheduler.dispatch_ready(), Some(waiter));
    assert!(
        matches!(scheduler.take_wakeup(waiter).unwrap(), Some(TaskWakeup::Task {
        completed, outcome: TaskOutcome::Completed(value),
    }) if completed == host && value == InterpValue::i32(42))
    );
    assert_eq!(scheduler.dispatch_ready(), None);
}

#[test]
fn completion_wakes_atomic_waiters_and_repeat_await_reads_cached_result() {
    let scope = TaskScopeId(3);
    let root = root_local(scope);
    let mut scheduler = TaskScheduler::new(scope, NonZeroUsize::new(2).unwrap());
    let target = spawn(&mut scheduler, &root, 0);
    let waiter = spawn(&mut scheduler, &root, 1);
    assert_eq!(scheduler.dispatch_ready(), Some(target));
    assert_eq!(scheduler.dispatch_ready(), Some(waiter));
    assert!(matches!(
        scheduler.await_task(waiter, target).unwrap(),
        AwaitDecision::Parked
    ));

    scheduler.complete(target, InterpValue::i32(42)).unwrap();
    assert_eq!(scheduler.state(target).unwrap(), TaskState::Completed);
    assert_eq!(scheduler.dispatch_ready(), Some(waiter));
    assert!(matches!(
        scheduler.take_wakeup(waiter).unwrap(),
        Some(TaskWakeup::Task {
            completed,
            outcome: TaskOutcome::Completed(InterpValue::Number(_)),
        }) if completed == target
    ));
    assert!(matches!(
        scheduler.await_task(waiter, target).unwrap(),
        AwaitDecision::Ready(TaskOutcome::Completed(InterpValue::Number(_)))
    ));
}

#[test]
fn dropped_handles_do_not_detach_tasks_and_primary_failure_uses_source_order() {
    let scope = TaskScopeId(9);
    let root = root_local(scope);
    let mut scheduler = TaskScheduler::new(scope, NonZeroUsize::new(2).unwrap());
    let later = spawn(&mut scheduler, &root, 2);
    let primary = spawn(&mut scheduler, &root, 1);
    let _ = scheduler.dispatch_ready().unwrap();
    let _ = scheduler.dispatch_ready().unwrap();
    scheduler
        .fail(
            later,
            RunFailure::ExecutionFault(crate::control::ExecutionFault::new(
                etas_core::AnalysisDiagnosticCode::InvalidArguments,
                etas_core::Span::empty(etas_core::SourceId(0), etas_core::TextSize::ZERO),
                "later",
            )),
        )
        .unwrap();
    scheduler
        .fail(
            primary,
            RunFailure::ExecutionFault(crate::control::ExecutionFault::new(
                etas_core::AnalysisDiagnosticCode::InvalidArguments,
                etas_core::Span::empty(etas_core::SourceId(0), etas_core::TextSize::ZERO),
                "primary",
            )),
        )
        .unwrap();

    assert_eq!(scheduler.primary_failure(), Some(primary));
    assert!(!scheduler.has_unsettled_tasks());
}

#[test]
fn continuation_tokens_are_owned_and_consumed_once() {
    let mut scheduler = TaskScheduler::new(TaskScopeId(12), NonZeroUsize::new(1).unwrap());
    let token = scheduler.allocate_continuation(
        TaskScopeId(12),
        TaskIdentity::Root,
        crate::control::Continuation::Return,
        etas_types::TypeId(0),
        None,
        None,
    );
    let consumed = scheduler
        .consume_continuation(token, TaskIdentity::Root, ContinuationTransfer::Resume)
        .unwrap();
    assert!(matches!(
        consumed.continuation,
        crate::control::Continuation::Return
    ));
    assert!(matches!(
        scheduler.consume_continuation(token, TaskIdentity::Root, ContinuationTransfer::Resume),
        Err(SchedulerError::ContinuationConsumed)
    ));

    let foreign = crate::value::ContinuationTokenValue::new(
        TaskScopeId(99),
        ContinuationId(token.continuation.0),
    );
    assert!(matches!(
        scheduler.consume_continuation(foreign, TaskIdentity::Root, ContinuationTransfer::Resume),
        Err(SchedulerError::WrongOwner)
    ));
}

#[test]
fn nested_scopes_share_ancestor_slots_and_waiting_parents_release_them() {
    let root_scope = TaskScopeId(20);
    let root = root_local(root_scope);
    let mut scheduler = TaskScheduler::new(root_scope, NonZeroUsize::new(1).unwrap());
    let parent = spawn(&mut scheduler, &root, 0);
    assert_eq!(scheduler.dispatch_ready(), Some(parent));

    let nested = scheduler.create_scope(root_scope, NonZeroUsize::new(2).unwrap());
    let first = scheduler
        .spawn_in_scope(&root, nested, EvalMachine::new(), None, 0, 1)
        .unwrap();
    let second = scheduler
        .spawn_in_scope(&root, nested, EvalMachine::new(), None, 0, 2)
        .unwrap();
    assert_eq!(scheduler.dispatch_ready(), None);

    let operation = root
        .execution
        .register(
            None,
            None,
            etas_host::TraceContext::root(etas_host::TraceId(2)),
        )
        .unwrap();
    let operation_id = operation.context().operation();
    scheduler.wait_for_host(parent, operation_id).unwrap();
    assert_eq!(scheduler.dispatch_ready(), Some(first));
    assert_eq!(scheduler.dispatch_ready(), None);
    scheduler.requeue(first).unwrap();
    assert_eq!(scheduler.dispatch_ready(), Some(second));
    operation
        .complete(
            etas_host::execution::ExternalOutcome::NotDispatched,
            Vec::new(),
        )
        .unwrap();
}

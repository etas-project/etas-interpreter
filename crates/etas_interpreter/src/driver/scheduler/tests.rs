use std::num::NonZeroUsize;

use super::*;
use crate::{
    api::ModelExecutionPolicy,
    eval::{TaskLocalState, machine::EvalMachine},
};

fn root_local(scope: TaskScopeId) -> TaskLocalState {
    TaskLocalState::root(
        scope,
        etas_host::execution::ExecutionScope::new_owned(),
        ModelExecutionPolicy::default(),
    )
}

fn spawn(
    scheduler: &mut TaskScheduler,
    parent: &TaskLocalState,
    source_index: usize,
) -> TaskHandle {
    scheduler
        .spawn(parent, EvalMachine::new(), None, 0, source_index)
        .unwrap()
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

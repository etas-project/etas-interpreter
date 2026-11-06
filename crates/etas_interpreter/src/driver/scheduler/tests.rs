use std::num::NonZeroUsize;

use super::*;
use crate::{
    api::ModelExecutionPolicy,
    eval::{TaskLocalState, machine::EvalMachine},
};

fn root_local() -> TaskLocalState {
    TaskLocalState::root(
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
    let root = root_local();
    let mut scheduler = TaskScheduler::new(TaskScopeId(7), NonZeroUsize::new(1).unwrap());
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
fn completion_wakes_atomic_waiters_and_repeat_await_reads_cached_result() {
    let root = root_local();
    let mut scheduler = TaskScheduler::new(TaskScopeId(3), NonZeroUsize::new(2).unwrap());
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
    let root = root_local();
    let mut scheduler = TaskScheduler::new(TaskScopeId(9), NonZeroUsize::new(2).unwrap());
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

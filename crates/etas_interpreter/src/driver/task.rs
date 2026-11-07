use etas_core::AnalysisDiagnosticCode;

use crate::{
    control::{
        Continuation, ContinuationInput, ControlSignal, PendingContinuation, PendingPerform,
        PendingTaskControl,
    },
    eval::{
        EvalContext,
        machine::{MachinePoll, PendingBoundary},
    },
    host::HostServices,
    orchestration::WorkflowEvent,
    value::InterpValue,
};

use super::{
    dispatch,
    drive::{EvaluationOutcome, evaluation_failure},
    scheduler::{
        AwaitDecision, CompletedTaskHost, HostOwner, ScheduledTask, SchedulerError, ScopeDecision,
        TaskHandle, TaskHostReactor, TaskOutcome, TaskResume, TaskWakeup,
    },
};

pub(super) fn awaited_task(perform: &PendingPerform) -> Option<TaskHandle> {
    let is_async_await = perform.action.action == "await"
        && perform
            .action
            .effect
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.name == "Async");
    if !is_async_await {
        return None;
    }
    match perform.args.as_slice() {
        [InterpValue::Task(task)] => Some(task.handle),
        _ => None,
    }
}

pub(super) async fn await_from_root<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
    target: TaskHandle,
    continuation: Continuation,
) -> Result<ControlSignal, EvaluationOutcome> {
    let outcome = await_outcome(eval, host, reactor, target).await?;
    signal_from_outcome(outcome, continuation)
}

pub(super) async fn control_from_root<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
    machine: &mut crate::eval::machine::EvalMachine,
    control: PendingTaskControl,
) -> Result<(), EvaluationOutcome> {
    match control {
        PendingTaskControl::Run { scope, span: _ } => {
            let outcome = await_scope_outcome(eval, host, reactor, scope).await?;
            record_scope_exit(eval, scope, &outcome);
            machine.resume(signal_from_outcome(outcome, Continuation::BlockValue)?);
            Ok(())
        }
        PendingTaskControl::Requeue { token, value, span } => {
            let consumed = eval
                .scheduler
                .consume_continuation(
                    token,
                    crate::driver::scheduler::TaskIdentity::Root,
                    crate::driver::scheduler::ContinuationTransfer::Requeue,
                )
                .map_err(|error| scheduler_failure(eval, error))?;
            eval.events.push(WorkflowEvent::ContinuationConsumed {
                scope: token.owner.0,
                continuation: token.continuation.0,
                task: None,
                kind: "requeue".to_owned(),
            });
            validate_continuation_value(eval, &value, consumed.output, span)?;
            machine.transfer_from(token, consumed.continuation, value);
            eval.events.push(WorkflowEvent::ContinuationTransferred {
                scope: token.owner.0,
                continuation: token.continuation.0,
                task: None,
                kind: "requeue".to_owned(),
            });
            tokio::task::yield_now().await;
            Ok(())
        }
        PendingTaskControl::Park { task, token, span } => {
            let consumed = eval
                .scheduler
                .consume_continuation(
                    token,
                    crate::driver::scheduler::TaskIdentity::Root,
                    crate::driver::scheduler::ContinuationTransfer::Park,
                )
                .map_err(|error| scheduler_failure(eval, error))?;
            eval.events.push(WorkflowEvent::ContinuationConsumed {
                scope: token.owner.0,
                continuation: token.continuation.0,
                task: None,
                kind: "park".to_owned(),
            });
            let outcome = await_outcome(eval, host, reactor, task).await?;
            match outcome {
                TaskOutcome::Completed(value) => {
                    validate_continuation_value(eval, &value, consumed.output, span)?;
                    machine.transfer_from(token, consumed.continuation, value);
                    eval.events.push(WorkflowEvent::ContinuationTransferred {
                        scope: token.owner.0,
                        continuation: token.continuation.0,
                        task: None,
                        kind: "park".to_owned(),
                    });
                    Ok(())
                }
                TaskOutcome::Raised(perform) => {
                    machine.resume(signal_from_outcome(
                        TaskOutcome::Raised(perform),
                        consumed.continuation,
                    )?);
                    Ok(())
                }
                TaskOutcome::Failed(failure) => Err(EvaluationOutcome::Failed(failure)),
                TaskOutcome::Cancelled(cause) => Err(EvaluationOutcome::Cancelled(cause)),
            }
        }
    }
}

async fn await_scope_outcome<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
    scope: crate::driver::scheduler::TaskScopeId,
) -> Result<TaskOutcome, EvaluationOutcome> {
    loop {
        match eval.scheduler.scope_outcome(scope) {
            Ok(Some(outcome)) => return Ok(outcome),
            Ok(None) => {}
            Err(error) => return Err(scheduler_failure(eval, error)),
        }
        match drive_one(eval, host, reactor).await {
            Ok(true) => {}
            Ok(false) => return Err(scheduler_deadlock(eval)),
            Err(outcome) => return Err(outcome),
        }
    }
}

pub(super) async fn settle<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
    mut root: EvaluationOutcome,
) -> EvaluationOutcome {
    if !matches!(root, EvaluationOutcome::Completed(_)) {
        if let Err(outcome) = cancel_and_drain(eval, host, reactor).await {
            return outcome;
        }
        return root;
    }

    while eval.scheduler.has_unsettled_tasks() || !eval.cpu_jobs.is_empty() {
        if let Some(failure) = primary_failure(eval) {
            root = failure;
            if let Err(outcome) = cancel_and_drain(eval, host, reactor).await {
                return outcome;
            }
            break;
        }
        match drive_one(eval, host, reactor).await {
            Ok(true) => {}
            Ok(false) => return scheduler_deadlock(eval),
            Err(outcome) => return outcome,
        }
    }
    primary_failure(eval).unwrap_or(root)
}

async fn cancel_and_drain<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
) -> Result<(), EvaluationOutcome> {
    eval.scheduler
        .cancel_unsettled(etas_host::execution::CancellationReason::ChildFailed)
        .map_err(|error| scheduler_failure(eval, error))?;
    record_cancellation_events(eval);
    while eval.scheduler.has_unsettled_tasks() || !eval.cpu_jobs.is_empty() {
        match drive_one(eval, host, reactor).await {
            Ok(true) => {}
            Ok(false) => return Err(scheduler_deadlock(eval)),
            Err(outcome) => return Err(outcome),
        }
    }
    Ok(())
}

pub(super) async fn await_root_host<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
) -> Result<CompletedTaskHost, EvaluationOutcome> {
    loop {
        if let Some(completed) = reactor.take_root_completed() {
            return Ok(completed);
        }
        if let Some(failure) = primary_failure(eval) {
            eval.scheduler
                .cancel_unsettled(etas_host::execution::CancellationReason::ChildFailed)
                .map_err(|error| scheduler_failure(eval, error))?;
            record_cancellation_events(eval);
            return Err(failure);
        }
        match drive_one(eval, host, reactor).await {
            Ok(true) => {}
            Ok(false) => return Err(scheduler_deadlock(eval)),
            Err(outcome) => return Err(outcome),
        }
    }
}

async fn await_outcome<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
    target: TaskHandle,
) -> Result<TaskOutcome, EvaluationOutcome> {
    loop {
        if let Some(failure) = primary_failure(eval) {
            if let Err(error) = eval
                .scheduler
                .cancel_unsettled(etas_host::execution::CancellationReason::ChildFailed)
            {
                return Err(scheduler_failure(eval, error));
            }
            record_cancellation_events(eval);
            return Err(failure);
        }
        match eval.scheduler.outcome(target) {
            Ok(Some(outcome)) => return Ok(outcome.clone()),
            Ok(None) => {}
            Err(error) => return Err(scheduler_failure(eval, error)),
        }
        match drive_one(eval, host, reactor).await {
            Ok(true) => {}
            Ok(false) => return Err(scheduler_deadlock(eval)),
            Err(outcome) => return Err(outcome),
        }
    }
}

async fn drive_one<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
) -> Result<bool, EvaluationOutcome> {
    if drain_cpu_completions(eval)? {
        return Ok(true);
    }
    if let Some((owner, operation)) = reactor.poll_ready().await {
        match owner {
            HostOwner::Root => return Ok(true),
            HostOwner::Task(handle) => wake_from_host(eval, handle, operation)?,
        }
    }
    let ready = eval.scheduler.dispatch_ready();
    record_resource_events(eval);
    let Some(handle) = ready else {
        if !eval.cpu_jobs.is_empty() {
            wait_for_cpu_or_host(eval, reactor).await?;
            return Ok(true);
        }
        let Some((owner, operation)) = reactor.wait_next().await else {
            return Ok(false);
        };
        if let HostOwner::Task(handle) = owner {
            wake_from_host(eval, handle, operation)?;
        }
        return Ok(true);
    };
    let wakeup = eval
        .scheduler
        .take_wakeup(handle)
        .map_err(|error| scheduler_failure(eval, error))?;
    let mut task = eval
        .scheduler
        .take_running(handle)
        .map_err(|error| scheduler_failure(eval, error))?;

    match wakeup {
        Some(TaskWakeup::Task {
            completed: _,
            outcome,
        }) => {
            eval.events.push(WorkflowEvent::TaskReady {
                scope: handle.owner.0,
                task: handle.task.0,
            });
            let continuation = task
                .waiting_continuation
                .take()
                .ok_or_else(|| task_fault(eval, "awakened task has no saved await continuation"))?;
            match continuation {
                TaskResume::Await(continuation) => {
                    match signal_from_outcome(outcome, continuation) {
                        Ok(signal) => task.machine.resume(signal),
                        Err(outcome) => {
                            finish_task(eval, handle, task, outcome)?;
                            return Ok(true);
                        }
                    }
                }
                TaskResume::Transfer {
                    token,
                    continuation,
                    output,
                    span,
                } => match outcome {
                    TaskOutcome::Completed(value) => {
                        if let Err(outcome) =
                            validate_continuation_value(eval, &value, output, span)
                        {
                            finish_task(eval, handle, task, outcome)?;
                            return Ok(true);
                        }
                        task.machine.transfer_from(token, continuation, value);
                    }
                    TaskOutcome::Raised(perform) => {
                        let Ok(signal) =
                            signal_from_outcome(TaskOutcome::Raised(perform), continuation)
                        else {
                            unreachable!("typed error produces a control signal")
                        };
                        task.machine.resume(signal);
                    }
                    TaskOutcome::Failed(failure) => {
                        finish_task(eval, handle, task, EvaluationOutcome::Failed(failure))?;
                        return Ok(true);
                    }
                    TaskOutcome::Cancelled(cause) => {
                        finish_task(eval, handle, task, EvaluationOutcome::Cancelled(cause))?;
                        return Ok(true);
                    }
                },
                TaskResume::Run(_) => {
                    return Err(task_fault(eval, "task wakeup has a scope continuation"));
                }
            }
        }
        Some(TaskWakeup::Scope { scope, outcome }) => {
            eval.events.push(WorkflowEvent::TaskReady {
                scope: handle.owner.0,
                task: handle.task.0,
            });
            let continuation = task
                .waiting_continuation
                .take()
                .ok_or_else(|| task_fault(eval, "scope wakeup has no saved run continuation"))?;
            let TaskResume::Run(continuation) = continuation else {
                return Err(task_fault(eval, "scope wakeup has a task continuation"));
            };
            record_scope_exit(eval, scope, &outcome);
            match signal_from_outcome(outcome, continuation) {
                Ok(signal) => task.machine.resume(signal),
                Err(outcome) => {
                    finish_task(eval, handle, task, outcome)?;
                    return Ok(true);
                }
            }
        }
        None => {}
    }

    if task.cpu_capsule.is_some() && eval.cpu_jobs.len() >= eval.cpu_executor.capacity().get() {
        eval.scheduler
            .restore_running(handle, task)
            .and_then(|()| eval.scheduler.requeue(handle))
            .map_err(|error| scheduler_failure(eval, error))?;
        wait_for_cpu_or_host(eval, reactor).await?;
        return Ok(true);
    }

    if let Some(mut capsule) = task.cpu_capsule.take() {
        let dispatch_sequence = eval.next_cpu_dispatch;
        eval.next_cpu_dispatch = dispatch_sequence
            .checked_add(1)
            .expect("CPU dispatch sequence exhausted");
        capsule.dispatch_sequence = dispatch_sequence;
        let cancellation = capsule.cancellation.clone();
        let executor = eval.cpu_executor.clone();
        let job = match executor.submit(capsule) {
            Ok(job) => job,
            Err(error) => {
                eval.events.push(WorkflowEvent::CpuExecutorFallback {
                    task: Some(handle.task.0),
                    message: format!("CPU executor submission failed: {error:?}"),
                });
                eval.scheduler
                    .restore_running(handle, task)
                    .and_then(|()| eval.scheduler.requeue(handle))
                    .map_err(|error| scheduler_failure(eval, error))?;
                return Ok(true);
            }
        };
        eval.scheduler
            .restore_running(handle, task)
            .and_then(|()| eval.scheduler.wait_for_cpu(handle, job))
            .map_err(|error| scheduler_failure(eval, error))?;
        assert!(
            eval.cpu_jobs
                .insert(
                    job,
                    crate::eval::CpuJobRecord {
                        task: handle,
                        cancellation,
                    },
                )
                .is_none(),
            "CPU job identity must be unique"
        );
        eval.events.push(WorkflowEvent::TaskDispatched {
            scope: handle.owner.0,
            task: handle.task.0,
            job: job.0,
            dispatch_sequence,
        });
        eval.events.push(WorkflowEvent::TaskWaitingCpu {
            scope: handle.owner.0,
            task: handle.task.0,
            job: job.0,
        });
        return Ok(true);
    }

    loop {
        let scope = task.machine.program_scope();
        let (program, handler_floor) = match scope {
            Some(scope) => (scope.program, scope.handler_floor),
            None => (
                task.program.clone().ok_or_else(|| {
                    task_fault(eval, "scheduled task has no checked execution program")
                })?,
                task.handler_floor,
            ),
        };
        let mut child =
            eval.scheduled_task_context(&program, &mut task.local, handle.task, handler_floor);
        let span = crate::diagnostics::item_span(child.checked, child.entry_item);
        let readiness = if handler_floor == 0 {
            crate::host::validate_host_readiness(&child.plan, host.availability(), span)
        } else {
            crate::host::validate_execution_support(&child.plan, span)
        };
        if !readiness.is_empty() {
            child.diagnostics.extend(readiness);
            let outcome = evaluation_failure(&mut child);
            drop(child);
            finish_task(eval, handle, task, outcome)?;
            return Ok(true);
        }

        if let Some(completed) = reactor.take_completed(handle) {
            let signal = match completed {
                CompletedTaskHost::Console(completed) => {
                    let Some(signal) = dispatch::console::finish(&mut child, completed) else {
                        let outcome = evaluation_failure(&mut child);
                        drop(child);
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    };
                    signal
                }
                CompletedTaskHost::Command(completed) => {
                    let Some(signal) =
                        dispatch::command::finish(&mut child, &mut task.machine, completed)
                    else {
                        let outcome = evaluation_failure(&mut child);
                        drop(child);
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    };
                    signal
                }
                CompletedTaskHost::Host(completed) => {
                    let Some(signal) =
                        dispatch::host::finish(&mut child, &mut task.machine, completed)
                    else {
                        let outcome = evaluation_failure(&mut child);
                        drop(child);
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    };
                    signal
                }
                CompletedTaskHost::Tool(completed) => {
                    dispatch::tool::finish(&mut child, &mut task.machine, completed);
                    continue;
                }
                CompletedTaskHost::Model(completed) => {
                    dispatch::model::finish(&mut child, &mut task.machine, completed);
                    continue;
                }
                CompletedTaskHost::Approval(completed) => {
                    let Some(signal) = dispatch::approval::finish(&mut child, completed) else {
                        let outcome = evaluation_failure(&mut child);
                        drop(child);
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    };
                    signal
                }
                CompletedTaskHost::Compiler(completed) => {
                    dispatch::compiler::finish(&mut child, completed)
                }
                CompletedTaskHost::Memory(completed) => {
                    let Some(signal) =
                        dispatch::memory::finish(&mut child, &mut task.machine, completed)
                    else {
                        let outcome = evaluation_failure(&mut child);
                        drop(child);
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    };
                    signal
                }
            };
            task.machine.resume(signal);
            continue;
        }

        let boundary = match task.machine.run_until_yield(&mut child) {
            MachinePoll::ProgramChanged => continue,
            MachinePoll::Complete(value) => {
                drop(child);
                finish_task(eval, handle, task, EvaluationOutcome::Completed(*value))?;
                return Ok(true);
            }
            MachinePoll::Cancelled(cause) => {
                drop(child);
                finish_task(eval, handle, task, EvaluationOutcome::Cancelled(cause))?;
                return Ok(true);
            }
            MachinePoll::Fault(fault) => {
                child.diagnostics.push(fault.clone().into_diagnostic());
                drop(child);
                finish_task(
                    eval,
                    handle,
                    task,
                    EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault)),
                )?;
                return Ok(true);
            }
            MachinePoll::CooperativeYield => {
                drop(child);
                eval.scheduler
                    .restore_running(handle, task)
                    .and_then(|()| eval.scheduler.requeue(handle))
                    .map_err(|error| scheduler_failure(eval, error))?;
                tokio::task::yield_now().await;
                return Ok(true);
            }
            MachinePoll::Yield(boundary) => boundary,
        };

        if let PendingBoundary::Task(control) = boundary {
            match *control {
                PendingTaskControl::Run { scope, span: _ } => {
                    task.waiting_continuation = Some(TaskResume::Run(Continuation::BlockValue));
                    drop(child);
                    eval.scheduler
                        .restore_running(handle, task)
                        .map_err(|error| scheduler_failure(eval, error))?;
                    match eval
                        .scheduler
                        .await_scope(handle, scope)
                        .map_err(|error| scheduler_failure(eval, error))?
                    {
                        ScopeDecision::Parked => return Ok(true),
                        ScopeDecision::Ready(outcome) => {
                            task = eval
                                .scheduler
                                .take_running(handle)
                                .map_err(|error| scheduler_failure(eval, error))?;
                            let continuation =
                                task.waiting_continuation.take().ok_or_else(|| {
                                    task_fault(eval, "ready task scope has no run continuation")
                                })?;
                            let TaskResume::Run(continuation) = continuation else {
                                return Err(task_fault(
                                    eval,
                                    "ready task scope has a task continuation",
                                ));
                            };
                            record_scope_exit(eval, scope, &outcome);
                            match signal_from_outcome(outcome, continuation) {
                                Ok(signal) => task.machine.resume(signal),
                                Err(outcome) => {
                                    finish_task(eval, handle, task, outcome)?;
                                    return Ok(true);
                                }
                            }
                            continue;
                        }
                    }
                }
                PendingTaskControl::Requeue { token, value, span } => {
                    let consumed = match child.scheduler.consume_continuation(
                        token,
                        crate::driver::scheduler::TaskIdentity::Scheduled(handle.task),
                        crate::driver::scheduler::ContinuationTransfer::Requeue,
                    ) {
                        Ok(consumed) => consumed,
                        Err(error) => {
                            task.machine
                                .resume(ControlSignal::runtime_fault(error.to_string(), span));
                            continue;
                        }
                    };
                    child.events.push(WorkflowEvent::ContinuationConsumed {
                        scope: token.owner.0,
                        continuation: token.continuation.0,
                        task: Some(handle.task.0),
                        kind: "requeue".to_owned(),
                    });
                    if let Err(outcome) =
                        validate_continuation_value(&mut child, &value, consumed.output, span)
                    {
                        drop(child);
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    }
                    task.machine
                        .transfer_from(token, consumed.continuation, value);
                    drop(child);
                    eval.scheduler
                        .restore_running(handle, task)
                        .and_then(|()| eval.scheduler.requeue(handle))
                        .map_err(|error| scheduler_failure(eval, error))?;
                    eval.events.push(WorkflowEvent::ContinuationTransferred {
                        scope: token.owner.0,
                        continuation: token.continuation.0,
                        task: Some(handle.task.0),
                        kind: "requeue".to_owned(),
                    });
                    tokio::task::yield_now().await;
                    return Ok(true);
                }
                PendingTaskControl::Park {
                    task: target,
                    token,
                    span,
                } => {
                    let consumed = match child.scheduler.consume_continuation(
                        token,
                        crate::driver::scheduler::TaskIdentity::Scheduled(handle.task),
                        crate::driver::scheduler::ContinuationTransfer::Park,
                    ) {
                        Ok(consumed) => consumed,
                        Err(error) => {
                            task.machine
                                .resume(ControlSignal::runtime_fault(error.to_string(), span));
                            continue;
                        }
                    };
                    child.events.push(WorkflowEvent::ContinuationConsumed {
                        scope: token.owner.0,
                        continuation: token.continuation.0,
                        task: Some(handle.task.0),
                        kind: "park".to_owned(),
                    });
                    task.waiting_continuation = Some(TaskResume::Transfer {
                        token,
                        continuation: consumed.continuation,
                        output: consumed.output,
                        span,
                    });
                    drop(child);
                    eval.scheduler
                        .restore_running(handle, task)
                        .map_err(|error| scheduler_failure(eval, error))?;
                    match eval
                        .scheduler
                        .await_task(handle, target)
                        .map_err(|error| scheduler_failure(eval, error))?
                    {
                        AwaitDecision::Parked => {
                            eval.events.push(WorkflowEvent::TaskParked {
                                scope: handle.owner.0,
                                task: handle.task.0,
                                waiting_on: target.task.0,
                            });
                            eval.events.push(WorkflowEvent::ContinuationTransferred {
                                scope: token.owner.0,
                                continuation: token.continuation.0,
                                task: Some(handle.task.0),
                                kind: "park".to_owned(),
                            });
                            return Ok(true);
                        }
                        AwaitDecision::Ready(outcome) => {
                            task = eval
                                .scheduler
                                .take_running(handle)
                                .map_err(|error| scheduler_failure(eval, error))?;
                            let continuation =
                                task.waiting_continuation.take().ok_or_else(|| {
                                    task_fault(eval, "ready parked task has no saved continuation")
                                })?;
                            let TaskResume::Transfer {
                                token,
                                continuation,
                                output,
                                span,
                            } = continuation
                            else {
                                return Err(task_fault(
                                    eval,
                                    "ready parked task has an await continuation",
                                ));
                            };
                            match outcome {
                                TaskOutcome::Completed(value) => {
                                    if let Err(outcome) =
                                        validate_continuation_value(eval, &value, output, span)
                                    {
                                        finish_task(eval, handle, task, outcome)?;
                                        return Ok(true);
                                    }
                                    task.machine.transfer_from(token, continuation, value);
                                    eval.events.push(WorkflowEvent::ContinuationTransferred {
                                        scope: token.owner.0,
                                        continuation: token.continuation.0,
                                        task: Some(handle.task.0),
                                        kind: "park".to_owned(),
                                    });
                                    continue;
                                }
                                TaskOutcome::Raised(perform) => {
                                    let Ok(signal) = signal_from_outcome(
                                        TaskOutcome::Raised(perform),
                                        continuation,
                                    ) else {
                                        unreachable!("typed error produces a control signal")
                                    };
                                    task.machine.resume(signal);
                                    continue;
                                }
                                TaskOutcome::Failed(failure) => {
                                    finish_task(
                                        eval,
                                        handle,
                                        task,
                                        EvaluationOutcome::Failed(failure),
                                    )?;
                                    return Ok(true);
                                }
                                TaskOutcome::Cancelled(cause) => {
                                    finish_task(
                                        eval,
                                        handle,
                                        task,
                                        EvaluationOutcome::Cancelled(cause),
                                    )?;
                                    return Ok(true);
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(diagnostic) =
            crate::host::validate_boundary_readiness(&boundary, host.availability())
        {
            child.diagnostics.push(diagnostic);
            let outcome = evaluation_failure(&mut child);
            drop(child);
            finish_task(eval, handle, task, outcome)?;
            return Ok(true);
        }

        if let PendingBoundary::Perform(perform) = boundary {
            if let Err(fault) = child.consume_residual_permit(&perform) {
                child.diagnostics.push(fault.clone().into_diagnostic());
                drop(child);
                finish_task(
                    eval,
                    handle,
                    task,
                    EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault)),
                )?;
                return Ok(true);
            }
            if let Some(target) = awaited_task(&perform) {
                match child.scheduler.outcome(target) {
                    Ok(Some(outcome)) => {
                        match signal_from_outcome(outcome.clone(), perform.continuation) {
                            Ok(signal) => task.machine.resume(signal),
                            Err(outcome) => {
                                drop(child);
                                finish_task(eval, handle, task, outcome)?;
                                return Ok(true);
                            }
                        }
                        continue;
                    }
                    Ok(None) => {
                        task.waiting_continuation = Some(TaskResume::Await(perform.continuation));
                        drop(child);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| match eval.scheduler.await_task(handle, target)? {
                                AwaitDecision::Parked => Ok(()),
                                AwaitDecision::Ready(_) => Err(SchedulerError::InvalidTransition),
                            })
                            .map_err(|error| scheduler_failure(eval, error))?;
                        eval.events.push(WorkflowEvent::TaskParked {
                            scope: handle.owner.0,
                            task: handle.task.0,
                            waiting_on: target.task.0,
                        });
                        return Ok(true);
                    }
                    Err(error) => {
                        task.machine.resume(ControlSignal::runtime_fault(
                            error.to_string(),
                            perform.span,
                        ));
                        continue;
                    }
                }
            }

            if perform.error_type.is_some() {
                drop(child);
                finish_raised_task(eval, handle, task, *perform)?;
                return Ok(true);
            }

            let signal = if let Some(signal) = child.default_agent_inference(&perform) {
                signal
            } else {
                match dispatch::compiler::begin(&mut child, host, *perform).await {
                    dispatch::compiler::CompilerDispatchStart::Complete(signal) => signal,
                    dispatch::compiler::CompilerDispatchStart::NotCompiler(perform) => {
                        match dispatch::approval::begin(
                            &mut child,
                            host,
                            *perform,
                            &mut task.machine,
                        ) {
                            dispatch::approval::ApprovalDispatchStart::Complete(Some(signal)) => {
                                signal
                            }
                            dispatch::approval::ApprovalDispatchStart::Complete(None) => {
                                let outcome = evaluation_failure(&mut child);
                                drop(child);
                                finish_task(eval, handle, task, outcome)?;
                                return Ok(true);
                            }
                            dispatch::approval::ApprovalDispatchStart::Pending(pending) => {
                                let operation = pending.operation_id();
                                drop(child);
                                eval.scheduler
                                    .restore_running(handle, task)
                                    .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                                    .map_err(|error| scheduler_failure(eval, error))?;
                                let registered =
                                    reactor.start_approval(HostOwner::Task(handle), pending);
                                assert_eq!(
                                    registered, operation,
                                    "registered host operation changed id"
                                );
                                return Ok(true);
                            }
                        }
                    }
                    dispatch::compiler::CompilerDispatchStart::Pending(pending) => {
                        let operation = pending.operation_id();
                        drop(child);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
                        let registered = reactor.start_compiler(HostOwner::Task(handle), pending);
                        assert_eq!(
                            registered, operation,
                            "registered host operation changed id"
                        );
                        return Ok(true);
                    }
                }
            };
            task.machine.resume(signal);
            continue;
        }

        let signal = match boundary {
            PendingBoundary::Model(model) => {
                match dispatch::model::begin(&mut child, host, *model, &mut task.machine).await {
                    dispatch::model::ModelDispatchStart::Complete(true) => continue,
                    dispatch::model::ModelDispatchStart::Complete(false) => {
                        let outcome = evaluation_failure(&mut child);
                        drop(child);
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    }
                    dispatch::model::ModelDispatchStart::Pending(pending) => {
                        let operation = pending.operation_id();
                        drop(child);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
                        let registered = reactor.start_model(HostOwner::Task(handle), pending);
                        assert_eq!(
                            registered, operation,
                            "registered host operation changed id"
                        );
                        return Ok(true);
                    }
                }
            }
            PendingBoundary::Tool(tool) => {
                match dispatch::tool::begin(&mut child, host, *tool, &mut task.machine).await {
                    dispatch::tool::ToolDispatchStart::Complete(true) => continue,
                    dispatch::tool::ToolDispatchStart::Complete(false) => {
                        let outcome = evaluation_failure(&mut child);
                        drop(child);
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    }
                    dispatch::tool::ToolDispatchStart::Pending(pending) => {
                        let operation = pending.operation_id();
                        drop(child);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
                        let registered = reactor.start_tool(HostOwner::Task(handle), pending);
                        assert_eq!(
                            registered, operation,
                            "registered host operation changed id"
                        );
                        return Ok(true);
                    }
                }
            }
            PendingBoundary::Memory(memory) => {
                match dispatch::memory::begin(&mut child, host, *memory, &mut task.machine).await {
                    dispatch::memory::MemoryDispatchStart::Complete(Some(signal)) => signal,
                    dispatch::memory::MemoryDispatchStart::Complete(None) => {
                        let outcome = evaluation_failure(&mut child);
                        drop(child);
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    }
                    dispatch::memory::MemoryDispatchStart::Pending(pending) => {
                        let operation = pending.operation_id();
                        drop(child);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
                        let registered = reactor.start_memory(HostOwner::Task(handle), pending);
                        assert_eq!(registered, operation);
                        return Ok(true);
                    }
                }
            }
            PendingBoundary::Session(session) => {
                dispatch::session::dispatch(&mut child, host, *session).await
            }
            PendingBoundary::Console(console) => {
                match dispatch::console::begin(&mut child, host, *console).await {
                    dispatch::console::ConsoleDispatchStart::Complete(Some(signal)) => signal,
                    dispatch::console::ConsoleDispatchStart::Complete(None) => {
                        let outcome = evaluation_failure(&mut child);
                        drop(child);
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    }
                    dispatch::console::ConsoleDispatchStart::Pending(pending) => {
                        let operation = pending.operation_id();
                        drop(child);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
                        let registered = reactor.start_console(HostOwner::Task(handle), pending);
                        assert_eq!(
                            registered, operation,
                            "registered host operation changed id"
                        );
                        return Ok(true);
                    }
                }
            }
            PendingBoundary::Command(command) => {
                match dispatch::command::begin(&mut child, host, *command, &mut task.machine).await
                {
                    dispatch::command::CommandDispatchStart::Complete(Some(signal)) => signal,
                    dispatch::command::CommandDispatchStart::Complete(None) => {
                        let outcome = evaluation_failure(&mut child);
                        drop(child);
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    }
                    dispatch::command::CommandDispatchStart::Pending(pending) => {
                        let operation = pending.operation_id();
                        drop(child);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
                        let registered = reactor.start_command(HostOwner::Task(handle), pending);
                        assert_eq!(
                            registered, operation,
                            "registered host operation changed id"
                        );
                        return Ok(true);
                    }
                }
            }
            PendingBoundary::Host(boundary) => {
                match dispatch::host::begin(&mut child, host, *boundary, &mut task.machine).await {
                    dispatch::host::HostBoundaryDispatchStart::Complete(Some(signal)) => signal,
                    dispatch::host::HostBoundaryDispatchStart::Complete(None) => {
                        let outcome = evaluation_failure(&mut child);
                        drop(child);
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    }
                    dispatch::host::HostBoundaryDispatchStart::Pending(pending) => {
                        let operation = pending.operation_id();
                        drop(child);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
                        let registered = reactor.start_host(HostOwner::Task(handle), pending);
                        assert_eq!(
                            registered, operation,
                            "registered host operation changed id"
                        );
                        return Ok(true);
                    }
                }
            }
            PendingBoundary::Perform(_) => unreachable!("perform handled above"),
            PendingBoundary::Task(_) => unreachable!("task control handled above"),
        };
        task.machine.resume(signal);
    }
}

async fn wait_for_cpu_or_host(
    eval: &mut EvalContext<'_>,
    reactor: &mut TaskHostReactor<'_>,
) -> Result<(), EvaluationOutcome> {
    let executor = eval.cpu_executor.clone();
    tokio::select! {
        _ = executor.completion_signal().notified() => {},
        completed = reactor.wait_next(), if reactor.has_pending() => {
            if let Some((HostOwner::Task(handle), operation)) = completed {
                wake_from_host(eval, handle, operation)?;
            }
        }
    }
    Ok(())
}

fn drain_cpu_completions(eval: &mut EvalContext<'_>) -> Result<bool, EvaluationOutcome> {
    let executor = eval.cpu_executor.clone();
    let mut completions = Vec::new();
    while let Some(completion) = executor.try_completion() {
        completions.push(completion);
    }
    completions.sort_by_key(|completion| completion.job);
    let had_completions = !completions.is_empty();
    for completion in completions {
        let Some(record) = eval.cpu_jobs.remove(&completion.job) else {
            return Err(task_fault(
                eval,
                format!("CPU worker completed unknown job {}", completion.job.0),
            ));
        };
        let completion_sequence = eval.next_cpu_completion;
        eval.next_cpu_completion = completion_sequence
            .checked_add(1)
            .expect("CPU completion sequence exhausted");
        let mut telemetry = crate::api::CpuTaskTelemetry {
            job: completion.job.0,
            worker: completion.metrics.worker,
            dispatch_sequence: completion.metrics.dispatch_sequence,
            completion_sequence,
            thread: completion.metrics.thread,
            elapsed: completion.metrics.elapsed,
            consumed_steps: completion.consumed_budget.steps,
            timings: completion.metrics.timings,
        };
        telemetry.timings.return_wait = completion.metrics.finished_at.elapsed();
        eval.events.push(WorkflowEvent::TaskCpuCompleted {
            scope: record.task.owner.0,
            task: record.task.task.0,
            job: completion.job.0,
            worker: completion.metrics.worker,
            dispatch_sequence: completion.metrics.dispatch_sequence,
            completion_sequence,
        });
        if let Err(crate::driver::cpu::CpuExecutionFailure::Infrastructure { worker, message }) =
            &completion.result
        {
            eval.events.push(WorkflowEvent::CpuWorkerFailed {
                worker: *worker,
                live_workers: executor
                    .health()
                    .iter()
                    .enumerate()
                    .filter(|(index, state)| {
                        *index != usize::from(worker.0)
                            && !matches!(state, crate::driver::cpu::CpuWorkerHealth::Exited(_))
                    })
                    .count(),
                message: message.clone(),
            });
        }
        if eval
            .scheduler
            .state(record.task)
            .map_err(|error| scheduler_failure(eval, error))?
            .is_terminal()
        {
            eval.events.cpu_telemetry(&telemetry);
            continue;
        }
        let task = eval
            .scheduler
            .take_cpu_completion(record.task, completion.job)
            .map_err(|error| scheduler_failure(eval, error))?;
        let outcome = match completion.result {
            Ok(value) => {
                let started = std::time::Instant::now();
                let value = value.thaw();
                telemetry.timings.output_thaw = started.elapsed();
                EvaluationOutcome::Completed(value)
            }
            Err(crate::driver::cpu::CpuExecutionFailure::Cancelled(reason)) => {
                task.local
                    .execution
                    .cancel_source()
                    .stop(reason)
                    .map_err(|error| task_fault(eval, error.to_string()))?;
                let cause = task
                    .local
                    .execution
                    .signal()
                    .and_then(|signal| signal.cause())
                    .map_err(|error| task_fault(eval, error.to_string()))?
                    .expect("cancelled CPU task publishes a cause");
                EvaluationOutcome::Cancelled(cause)
            }
            Err(crate::driver::cpu::CpuExecutionFailure::ExecutionFault(fault)) => {
                eval.diagnostics.push(fault.clone().into_diagnostic());
                EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault))
            }
            Err(crate::driver::cpu::CpuExecutionFailure::ContractViolation(violation)) => {
                task_fault(eval, format!("CPU task contract violation: {violation:?}"))
            }
            Err(crate::driver::cpu::CpuExecutionFailure::Infrastructure { worker, message }) => {
                EvaluationOutcome::Failed(crate::api::RunFailure::CpuInfrastructure {
                    worker,
                    message,
                })
            }
        };
        eval.events.cpu_telemetry(&telemetry);
        finish_task(eval, record.task, task, outcome)?;
    }
    Ok(had_completions)
}

fn wake_from_host(
    eval: &mut EvalContext<'_>,
    handle: TaskHandle,
    operation: etas_host::execution::OperationId,
) -> Result<(), EvaluationOutcome> {
    eval.scheduler
        .wake_from_host(handle, operation)
        .map_err(|error| scheduler_failure(eval, error))?;
    eval.events.push(WorkflowEvent::TaskReady {
        scope: handle.owner.0,
        task: handle.task.0,
    });
    Ok(())
}

fn signal_from_outcome(
    outcome: TaskOutcome,
    continuation: Continuation,
) -> Result<ControlSignal, EvaluationOutcome> {
    match outcome {
        TaskOutcome::Completed(value) => {
            Ok(ControlSignal::pending_continuation(PendingContinuation {
                continuation,
                input: ContinuationInput::Value(value),
            }))
        }
        TaskOutcome::Raised(mut perform) => {
            perform.continuation = continuation;
            Ok(ControlSignal::pending_perform(*perform))
        }
        TaskOutcome::Failed(failure) => Err(EvaluationOutcome::Failed(failure)),
        TaskOutcome::Cancelled(cause) => Err(EvaluationOutcome::Cancelled(cause)),
    }
}

fn finish_task(
    eval: &mut EvalContext<'_>,
    handle: TaskHandle,
    task: ScheduledTask,
    outcome: EvaluationOutcome,
) -> Result<(), EvaluationOutcome> {
    // Both execution modes commit values through this coordinator boundary.
    // Resource handles stay local; data values use the parent's storage limits.
    let outcome = match outcome {
        EvaluationOutcome::Completed(value) => {
            let error = crate::eval::interp_to_host_value(&value)
                .ok()
                .and_then(|projected| eval.storage_limits.value_size(&projected).err());
            match error {
                Some(error) => {
                    let fault = crate::control::ExecutionFault::new(
                        AnalysisDiagnosticCode::UnhandledRuntimeError,
                        crate::diagnostics::item_span(eval.checked, eval.entry_item),
                        format!(
                            "task result exceeds the run storage limits: {}",
                            error.message
                        ),
                    );
                    eval.diagnostics.push(fault.clone().into_diagnostic());
                    EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault))
                }
                None => EvaluationOutcome::Completed(value),
            }
        }
        other => other,
    };
    let outcome_name = match &outcome {
        EvaluationOutcome::Completed(_) => "completed",
        EvaluationOutcome::Failed(_) => "failed",
        EvaluationOutcome::Cancelled(_) => "cancelled",
    };
    eval.scheduler
        .restore_running(handle, task)
        .map_err(|error| scheduler_failure(eval, error))?;
    let result = match outcome {
        EvaluationOutcome::Completed(value) => eval.scheduler.complete(handle, value),
        EvaluationOutcome::Failed(failure) => eval.scheduler.fail(handle, failure),
        EvaluationOutcome::Cancelled(cause) => eval.scheduler.cancelled(handle, cause),
    };
    result
        .and_then(|()| eval.scheduler.apply_group_policy(handle))
        .map_err(|error| scheduler_failure(eval, error))?;
    record_cancellation_events(eval);
    eval.events.push(WorkflowEvent::TaskSettled {
        scope: handle.owner.0,
        task: handle.task.0,
        outcome: outcome_name.to_owned(),
    });
    eval.events.push(WorkflowEvent::TaskCleanupCompleted {
        scope: handle.owner.0,
        task: handle.task.0,
    });
    Ok(())
}

fn finish_raised_task(
    eval: &mut EvalContext<'_>,
    handle: TaskHandle,
    task: ScheduledTask,
    perform: PendingPerform,
) -> Result<(), EvaluationOutcome> {
    eval.scheduler
        .restore_running(handle, task)
        .and_then(|()| eval.scheduler.raise(handle, perform))
        .and_then(|()| eval.scheduler.apply_group_policy(handle))
        .map_err(|error| scheduler_failure(eval, error))?;
    record_cancellation_events(eval);
    eval.events.push(WorkflowEvent::TaskSettled {
        scope: handle.owner.0,
        task: handle.task.0,
        outcome: "raised".to_owned(),
    });
    eval.events.push(WorkflowEvent::TaskCleanupCompleted {
        scope: handle.owner.0,
        task: handle.task.0,
    });
    Ok(())
}

fn record_scope_exit(
    eval: &mut EvalContext<'_>,
    scope: crate::driver::scheduler::TaskScopeId,
    outcome: &TaskOutcome,
) {
    let (decision, task) = eval
        .scheduler
        .scope_resolution(scope)
        .expect("completed task scope has a resolution");
    eval.events.push(WorkflowEvent::ScopeResolved {
        scope: scope.0,
        decision: decision.to_owned(),
        task: task.map(|task| task.0),
    });
    let budget = eval
        .host_context
        .budget
        .snapshot()
        .expect("task scope budget observation succeeds");
    eval.events.push(WorkflowEvent::ScopeBudgetObserved {
        scope: scope.0,
        consumed_tokens: budget.consumed_tokens,
        consumed_cost_micros: budget.consumed_cost_micros,
    });
    let outcome = match outcome {
        TaskOutcome::Completed(_) => "completed",
        TaskOutcome::Raised(_) => "raised",
        TaskOutcome::Failed(_) => "failed",
        TaskOutcome::Cancelled(_) => "cancelled",
    };
    eval.events.push(WorkflowEvent::ScopeExited {
        scope: scope.0,
        outcome: outcome.to_owned(),
    });
}

fn record_cancellation_events(eval: &mut EvalContext<'_>) {
    record_resource_events(eval);
    for (handle, reason) in eval.scheduler.take_cancellation_events() {
        for (job, record) in &eval.cpu_jobs {
            if record.task == handle {
                record
                    .cancellation
                    .store(true, std::sync::atomic::Ordering::Release);
                eval.cpu_executor.cancel(*job, reason.clone());
            }
        }
        eval.events.push(WorkflowEvent::TaskCancellationRequested {
            scope: handle.owner.0,
            task: handle.task.0,
            reason: format!("{reason:?}"),
        });
    }
}

fn record_resource_events(eval: &mut EvalContext<'_>) {
    use crate::driver::scheduler::ResourceEvent;
    for event in eval.scheduler.take_resource_events() {
        let task = match event {
            ResourceEvent::Acquired(task, _)
            | ResourceEvent::Waiting(task, _)
            | ResourceEvent::Released(task, _) => task,
        };
        let scope = eval.scheduler.task_scope(task).0;
        eval.events.push(match event {
            ResourceEvent::Acquired(task, permits) => WorkflowEvent::TaskResourcesAcquired {
                scope,
                task: task.0,
                permits,
            },
            ResourceEvent::Waiting(task, guard) => WorkflowEvent::TaskWaitingResource {
                scope,
                task: task.0,
                guard: guard.0,
            },
            ResourceEvent::Released(task, permits) => WorkflowEvent::TaskResourcesReleased {
                scope,
                task: task.0,
                permits,
            },
        });
    }
}

fn primary_failure(eval: &mut EvalContext<'_>) -> Option<EvaluationOutcome> {
    let handle = eval.scheduler.primary_failure()?;
    match eval.scheduler.outcome(handle) {
        Ok(Some(TaskOutcome::Failed(failure))) => Some(EvaluationOutcome::Failed(failure.clone())),
        _ => None,
    }
}

fn scheduler_deadlock(eval: &mut EvalContext<'_>) -> EvaluationOutcome {
    task_fault(
        eval,
        "structured task scope has no runnable task but still owns unfinished work",
    )
}

fn scheduler_failure(eval: &mut EvalContext<'_>, error: SchedulerError) -> EvaluationOutcome {
    task_fault(eval, error.to_string())
}

fn validate_continuation_value(
    eval: &mut EvalContext<'_>,
    value: &InterpValue,
    expected: etas_types::TypeId,
    span: etas_core::Span,
) -> Result<(), EvaluationOutcome> {
    crate::value::contract::runtime::validate_value(
        eval.checked,
        eval.plan,
        value,
        expected,
        &eval.storage_limits,
    )
    .map_err(|reason| {
        let fault = crate::control::ExecutionFault::new(
            AnalysisDiagnosticCode::InvalidArguments,
            span,
            format!("continuation transfer value violates its checked type: {reason}"),
        );
        eval.diagnostics.push(fault.clone().into_diagnostic());
        EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault))
    })
}

fn task_fault(eval: &mut EvalContext<'_>, message: impl Into<String>) -> EvaluationOutcome {
    let span = crate::diagnostics::item_span(eval.checked, eval.entry_item);
    let fault = crate::control::ExecutionFault::new(
        AnalysisDiagnosticCode::MissingCheckedFact,
        span,
        message,
    );
    eval.diagnostics.push(fault.clone().into_diagnostic());
    EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault))
}

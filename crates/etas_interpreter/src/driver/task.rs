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
        AwaitDecision, CompletedTaskHost, ScheduledTask, SchedulerError, TaskHandle,
        TaskHostReactor, TaskOutcome, TaskResume, TaskWakeup,
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
        PendingTaskControl::Requeue { token, value, span } => {
            let consumed = eval
                .scheduler
                .consume_continuation(
                    token,
                    crate::driver::scheduler::TaskIdentity::Root,
                    crate::driver::scheduler::ContinuationTransfer::Requeue,
                )
                .map_err(|error| scheduler_failure(eval, error))?;
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
                TaskOutcome::Failed(failure) => Err(EvaluationOutcome::Failed(failure)),
                TaskOutcome::Cancelled(cause) => Err(EvaluationOutcome::Cancelled(cause)),
            }
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
        if let Err(error) = eval
            .scheduler
            .cancel_unsettled(etas_host::execution::CancellationReason::ChildFailed)
        {
            return scheduler_failure(eval, error);
        }
        return root;
    }

    while eval.scheduler.has_unsettled_tasks() {
        if let Some(failure) = primary_failure(eval) {
            root = failure;
            if let Err(error) = eval
                .scheduler
                .cancel_unsettled(etas_host::execution::CancellationReason::ChildFailed)
            {
                return scheduler_failure(eval, error);
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
    if let Some((handle, operation)) = reactor.poll_ready().await {
        wake_from_host(eval, handle, operation)?;
    }
    let Some(handle) = eval.scheduler.dispatch_ready() else {
        let Some((handle, operation)) = reactor.wait_next().await else {
            return Ok(false);
        };
        wake_from_host(eval, handle, operation)?;
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

    if let Some(TaskWakeup::Task {
        completed: _,
        outcome,
    }) = wakeup
    {
        eval.events.push(WorkflowEvent::TaskReady {
            scope: handle.owner.0,
            task: handle.task.0,
        });
        let continuation = task
            .waiting_continuation
            .take()
            .ok_or_else(|| task_fault(eval, "awakened task has no saved await continuation"))?;
        match continuation {
            TaskResume::Await(continuation) => match signal_from_outcome(outcome, continuation) {
                Ok(signal) => task.machine.resume(signal),
                Err(outcome) => {
                    finish_task(eval, handle, task, outcome)?;
                    return Ok(true);
                }
            },
            TaskResume::Transfer {
                token,
                continuation,
                output,
                span,
            } => match outcome {
                TaskOutcome::Completed(value) => {
                    if let Err(outcome) = validate_continuation_value(eval, &value, output, span) {
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    }
                    task.machine.transfer_from(token, continuation, value);
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
        }
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

            let signal = if let Some(signal) = child.default_agent_inference(&perform) {
                signal
            } else {
                match dispatch::compiler::dispatch(&mut child, host, *perform).await {
                    Ok(signal) => signal,
                    Err(perform) => {
                        let Some(signal) = dispatch::approval::dispatch(
                            &mut child,
                            host,
                            *perform,
                            &mut task.machine,
                        )
                        .await
                        else {
                            let outcome = evaluation_failure(&mut child);
                            drop(child);
                            finish_task(eval, handle, task, outcome)?;
                            return Ok(true);
                        };
                        signal
                    }
                }
            };
            task.machine.resume(signal);
            continue;
        }

        let signal = match boundary {
            PendingBoundary::Model(model) => {
                if !dispatch::model::dispatch(&mut child, host, &model, &mut task.machine).await {
                    let outcome = evaluation_failure(&mut child);
                    drop(child);
                    finish_task(eval, handle, task, outcome)?;
                    return Ok(true);
                }
                continue;
            }
            PendingBoundary::Tool(tool) => {
                if !dispatch::tool::dispatch(&mut child, host, *tool, &mut task.machine).await {
                    let outcome = evaluation_failure(&mut child);
                    drop(child);
                    finish_task(eval, handle, task, outcome)?;
                    return Ok(true);
                }
                continue;
            }
            PendingBoundary::Memory(memory) => {
                let Some(signal) =
                    dispatch::memory::dispatch(&mut child, host, *memory, &mut task.machine).await
                else {
                    let outcome = evaluation_failure(&mut child);
                    drop(child);
                    finish_task(eval, handle, task, outcome)?;
                    return Ok(true);
                };
                signal
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
                        let registered = reactor.start_console(handle, pending);
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
                        let registered = reactor.start_command(handle, pending);
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
                        let registered = reactor.start_host(handle, pending);
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
    result.map_err(|error| scheduler_failure(eval, error))?;
    eval.events.push(WorkflowEvent::TaskSettled {
        scope: handle.owner.0,
        task: handle.task.0,
        outcome: outcome_name.to_owned(),
    });
    Ok(())
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

use etas_core::AnalysisDiagnosticCode;

use crate::{
    control::{
        Continuation, ContinuationInput, ControlSignal, PendingContinuation, PendingPerform,
    },
    eval::{
        EvalContext,
        machine::{MachinePoll, PendingBoundary},
    },
    host::HostServices,
    value::InterpValue,
};

use super::{
    dispatch,
    drive::{EvaluationOutcome, evaluation_failure},
    scheduler::{
        AwaitDecision, ScheduledTask, SchedulerError, TaskHandle, TaskOutcome, TaskWakeup,
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

pub(super) async fn await_from_root(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    target: TaskHandle,
    continuation: Continuation,
) -> Result<ControlSignal, EvaluationOutcome> {
    let outcome = await_outcome(eval, host, target).await?;
    signal_from_outcome(outcome, continuation)
}

pub(super) async fn settle(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
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
        match drive_one(eval, host).await {
            Ok(true) => {}
            Ok(false) => return scheduler_deadlock(eval),
            Err(outcome) => return outcome,
        }
    }
    primary_failure(eval).unwrap_or(root)
}

async fn await_outcome(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
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
        match drive_one(eval, host).await {
            Ok(true) => {}
            Ok(false) => return Err(scheduler_deadlock(eval)),
            Err(outcome) => return Err(outcome),
        }
    }
}

async fn drive_one(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
) -> Result<bool, EvaluationOutcome> {
    let Some(handle) = eval.scheduler.dispatch_ready() else {
        return Ok(false);
    };
    let wakeup = eval
        .scheduler
        .take_wakeup(handle)
        .map_err(|error| scheduler_failure(eval, error))?;
    let mut task = eval
        .scheduler
        .take_running(handle)
        .map_err(|error| scheduler_failure(eval, error))?;

    if let Some(TaskWakeup::Task { outcome, .. }) = wakeup {
        let continuation = task
            .waiting_continuation
            .take()
            .ok_or_else(|| task_fault(eval, "awakened task has no saved await continuation"))?;
        match signal_from_outcome(outcome, continuation) {
            Ok(signal) => task.machine.resume(signal),
            Err(outcome) => {
                finish_task(eval, handle, task, outcome)?;
                return Ok(true);
            }
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
        let mut child = eval.scheduled_task_context(&program, &mut task.local, handler_floor);
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
                        task.waiting_continuation = Some(perform.continuation);
                        drop(child);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| match eval.scheduler.await_task(handle, target)? {
                                AwaitDecision::Parked => Ok(()),
                                AwaitDecision::Ready(_) => Err(SchedulerError::InvalidTransition),
                            })
                            .map_err(|error| scheduler_failure(eval, error))?;
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
                let Some(signal) = dispatch::console::dispatch(&mut child, host, *console).await
                else {
                    let outcome = evaluation_failure(&mut child);
                    drop(child);
                    finish_task(eval, handle, task, outcome)?;
                    return Ok(true);
                };
                signal
            }
            PendingBoundary::Command(command) => {
                let Some(signal) =
                    dispatch::command::dispatch(&mut child, host, *command, &mut task.machine)
                        .await
                else {
                    let outcome = evaluation_failure(&mut child);
                    drop(child);
                    finish_task(eval, handle, task, outcome)?;
                    return Ok(true);
                };
                signal
            }
            PendingBoundary::Host(boundary) => {
                let Some(signal) =
                    dispatch::host::dispatch(&mut child, host, *boundary, &mut task.machine).await
                else {
                    let outcome = evaluation_failure(&mut child);
                    drop(child);
                    finish_task(eval, handle, task, outcome)?;
                    return Ok(true);
                };
                signal
            }
            PendingBoundary::Perform(_) => unreachable!("perform handled above"),
        };
        task.machine.resume(signal);
    }
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
    eval.scheduler
        .restore_running(handle, task)
        .map_err(|error| scheduler_failure(eval, error))?;
    let result = match outcome {
        EvaluationOutcome::Completed(value) => eval.scheduler.complete(handle, value),
        EvaluationOutcome::Failed(failure) => eval.scheduler.fail(handle, failure),
        EvaluationOutcome::Cancelled(cause) => eval.scheduler.cancelled(handle, cause),
    };
    result.map_err(|error| scheduler_failure(eval, error))
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

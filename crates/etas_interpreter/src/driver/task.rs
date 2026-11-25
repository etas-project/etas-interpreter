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
        AwaitDecision, CompletedTaskHost, CompletionDecision, HostOwner, ReadyTaskHost,
        RootTaskWait, ScheduledTask, SchedulerError, ScopeDecision, TaskHandle, TaskHostReactor,
        TaskOutcome, TaskResume, TaskState, TaskWakeup,
    },
};

pub(super) fn awaited_task(eval: &EvalContext<'_>, perform: &PendingPerform) -> Option<TaskHandle> {
    if eval.checked.hir.source_standard_types {
        let [task] = perform.args.as_slice() else {
            return None;
        };
        if !eval.source_compiler_default(perform, etas_std::CompilerActionRole::TaskAwait) {
            return None;
        }
        return crate::value::task::source::task(task, eval.checked).map(|task| task.handle);
    }
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
    machine: &mut crate::eval::machine::EvalMachine,
    target: TaskHandle,
    continuation: Continuation,
) -> Result<(), EvaluationOutcome> {
    machine.root_wait = Some(RootTaskWait::Await {
        target,
        continuation,
    });
    resume_root_wait(eval, host, reactor, machine).await
}

pub(super) async fn resume_root_wait<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
    machine: &mut crate::eval::machine::EvalMachine,
) -> Result<(), EvaluationOutcome> {
    if machine
        .abandon_cancelled_agent_handler(eval)
        .map_err(|fault| EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault)))?
    {
        return Ok(());
    }
    if matches!(machine.root_wait, Some(RootTaskWait::Ready(_))) {
        let Some(RootTaskWait::Ready(input)) = machine.root_wait.take() else {
            unreachable!()
        };
        machine.restore_task_input(*input);
        return Ok(());
    }
    let outcome = match machine.root_wait.as_ref().expect("root has a pending wait") {
        RootTaskWait::Ready(_) => unreachable!(),
        RootTaskWait::Completion { .. } => {
            return Err(task_fault(
                eval,
                "completion wait belongs to its scheduled scope owner",
            ));
        }
        RootTaskWait::Await { target, .. } | RootTaskWait::Transfer { target, .. } => {
            await_outcome(eval, host, reactor, *target, machine).await?
        }
        RootTaskWait::Run { scope } => {
            await_scope_outcome(eval, host, reactor, *scope, machine).await?
        }
        RootTaskWait::Host(_) => return await_root_host(eval, host, reactor, machine).await,
        RootTaskWait::Settling(_) => {
            unreachable!("root settlement is driven after body completion")
        }
    };
    let Some(outcome) = outcome else {
        return Ok(());
    };
    match machine
        .root_wait
        .take()
        .expect("task wait remains owned by the root machine")
    {
        RootTaskWait::Await { continuation, .. } => {
            machine.resume(signal_from_outcome(outcome, continuation)?)
        }
        RootTaskWait::Run { scope } => {
            record_scope_exit(eval, scope, &outcome);
            machine.resume(signal_from_outcome(outcome, Continuation::BlockValue)?);
        }
        RootTaskWait::Transfer {
            token,
            continuation,
            output,
            span,
            ..
        } => {
            if let TaskOutcome::Completed(value) = &outcome {
                validate_continuation_value(eval, token, value, output, span)?;
                eval.events.push(WorkflowEvent::ContinuationTransferred {
                    scope: token.owner.0,
                    continuation: token.continuation.0,
                    task: None,
                    kind: "park".to_owned(),
                });
            }
            finish_parked_transfer(machine, eval.task, token, continuation, outcome, span)?;
        }
        RootTaskWait::Host(_)
        | RootTaskWait::Settling(_)
        | RootTaskWait::Ready(_)
        | RootTaskWait::Completion { .. } => {
            unreachable!("root wait kind is unchanged")
        }
    }
    Ok(())
}

fn start_managed<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    control: PendingTaskControl,
) -> dispatch::managed_agent::ManagedDispatchStart<'host> {
    match control {
        PendingTaskControl::ManagedAgent(pending) => {
            dispatch::managed_agent::begin(eval, host, *pending)
        }
        PendingTaskControl::ManagedAuthorization(pending) => {
            dispatch::managed_agent::advance(eval, host, *pending)
        }
        _ => unreachable!("managed admission control"),
    }
}

pub(super) async fn control_from_root<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
    machine: &mut crate::eval::machine::EvalMachine,
    control: PendingTaskControl,
) -> Result<(), EvaluationOutcome> {
    match control {
        PendingTaskControl::NativeAuthorization(pending) => {
            match dispatch::authorization::advance(eval, host, *pending) {
                dispatch::authorization::AuthorizationStart::Complete(signal) => {
                    machine.resume(signal)
                }
                dispatch::authorization::AuthorizationStart::Pending(pending) => {
                    machine.root_wait = Some(RootTaskWait::Host(
                        reactor.start_authorization(HostOwner::Root, pending),
                    ));
                    return await_root_host(eval, host, reactor, machine).await;
                }
            }
            Ok(())
        }
        PendingTaskControl::AuthorizedNative(pending) => {
            match Box::pin(dispatch::authorization::dispatch(
                eval, host, machine, *pending,
            ))
            .await
            {
                dispatch::authorization::AuthorizedDispatchStart::Signal(signal) => {
                    machine.resume(signal)
                }
                dispatch::authorization::AuthorizedDispatchStart::Ready => {}
                dispatch::authorization::AuthorizedDispatchStart::Failed => {
                    return Err(evaluation_failure(eval));
                }
                dispatch::authorization::AuthorizedDispatchStart::Pending(pending) => {
                    machine.root_wait = Some(RootTaskWait::Host(
                        reactor.start_authorized(HostOwner::Root, pending),
                    ));
                    return await_root_host(eval, host, reactor, machine).await;
                }
            }
            Ok(())
        }
        control @ (PendingTaskControl::ManagedAgent(_)
        | PendingTaskControl::ManagedAuthorization(_)) => {
            match start_managed(eval, host, control) {
                dispatch::managed_agent::ManagedDispatchStart::Complete(signal) => {
                    machine.resume(signal)
                }
                dispatch::managed_agent::ManagedDispatchStart::Pending(pending) => {
                    machine.root_wait = Some(RootTaskWait::Host(
                        reactor.start_managed(HostOwner::Root, pending),
                    ));
                    return await_root_host(eval, host, reactor, machine).await;
                }
            }
            Ok(())
        }
        PendingTaskControl::AgentReply {
            task, continuation, ..
        } => await_from_root(eval, host, reactor, machine, task, continuation).await,
        PendingTaskControl::NextCompletion { .. } => Err(task_fault(
            eval,
            "completion observation requires a scheduled scope owner",
        )),
        control @ (PendingTaskControl::ResumeHandled { .. }
        | PendingTaskControl::RestoreHandlerWait { .. }) => {
            if let PendingTaskControl::ResumeHandled {
                task,
                handler,
                value,
                ..
            } = control
            {
                eval.scheduler
                    .resume_handled_action(task, handler, value)
                    .map_err(|error| scheduler_failure(eval, error))?;
            }
            resume_root_wait(eval, host, reactor, machine).await
        }
        PendingTaskControl::Run { scope, .. } => {
            machine.root_wait = Some(RootTaskWait::Run { scope });
            resume_root_wait(eval, host, reactor, machine).await
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
            validate_continuation_value(eval, token, &value, consumed.output, span)?;
            machine.transfer_from(eval.task, token, consumed.continuation, value);
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
            let continuation = machine
                .park_transfer(eval, token, consumed.continuation, span)
                .map_err(|fault| {
                    EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault))
                })?;
            machine.root_wait = Some(RootTaskWait::Transfer {
                target: task,
                token,
                continuation,
                output: consumed.output,
                span,
            });
            resume_root_wait(eval, host, reactor, machine).await
        }
    }
}

async fn await_scope_outcome<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
    scope: crate::driver::scheduler::TaskScopeId,
    machine: &mut crate::eval::machine::EvalMachine,
) -> Result<Option<TaskOutcome>, EvaluationOutcome> {
    loop {
        if machine
            .abandon_cancelled_agent_handler(eval)
            .map_err(|fault| {
                EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault))
            })?
        {
            return Ok(None);
        }
        match eval.scheduler.scope_outcome(scope) {
            Ok(Some(outcome)) => return Ok(Some(outcome)),
            Ok(None) => {}
            Err(error) => return Err(scheduler_failure(eval, error)),
        }
        match drive_one_with_root(eval, host, reactor, Some(machine)).await {
            Ok(true) => {
                if machine.root_wait.is_none() {
                    return Ok(None);
                }
            }
            Ok(false) => return Err(scheduler_deadlock(eval)),
            Err(outcome) => return Err(outcome),
        }
    }
}

pub(super) async fn settle<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
    outcome: EvaluationOutcome,
    machine: &mut crate::eval::machine::EvalMachine,
) -> EvaluationOutcome {
    machine.root_wait = Some(RootTaskWait::Settling(match &outcome {
        EvaluationOutcome::Completed(value) => TaskOutcome::Completed((**value).clone()),
        EvaluationOutcome::Failed(failure) => TaskOutcome::Failed(failure.clone()),
        EvaluationOutcome::Cancelled(cause) => TaskOutcome::Cancelled(cause.clone()),
    }));
    let outcome = settle_tasks(eval, host, reactor, outcome, machine).await;
    machine.root_wait = None;
    outcome
}

pub(super) fn take_root_settlement(
    machine: &mut crate::eval::machine::EvalMachine,
) -> Option<EvaluationOutcome> {
    if !matches!(machine.root_wait, Some(RootTaskWait::Settling(_))) {
        return None;
    }
    let RootTaskWait::Settling(outcome) = machine.root_wait.take().expect("root is settling")
    else {
        unreachable!()
    };
    Some(match outcome {
        TaskOutcome::Completed(value) => EvaluationOutcome::Completed(Box::new(value)),
        TaskOutcome::Failed(failure) => EvaluationOutcome::Failed(failure),
        TaskOutcome::Cancelled(cause) => EvaluationOutcome::Cancelled(cause),
        TaskOutcome::Raised(_) => {
            unreachable!("root typed errors are dispatched before settlement")
        }
    })
}

#[cfg(test)]
pub(super) async fn drive_one<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
) -> Result<bool, EvaluationOutcome> {
    drive_one_with_root(eval, host, reactor, None).await
}

async fn settle_tasks<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
    mut root: EvaluationOutcome,
    machine: &mut crate::eval::machine::EvalMachine,
) -> EvaluationOutcome {
    if let Err(error) = eval
        .scheduler
        .close_agent_scope(crate::driver::scheduler::TaskScopeId(0))
    {
        return scheduler_failure(eval, error);
    }
    eval.task.agent_initializations.clear();
    record_cancellation_events(eval);
    if !matches!(root, EvaluationOutcome::Completed(_)) {
        if let Err(outcome) = cancel_and_drain(eval, host, reactor, machine).await {
            return outcome;
        }
        return root;
    }

    while eval.scheduler.has_unsettled_tasks() || !eval.cpu_jobs.is_empty() || reactor.has_pending()
    {
        if let Some(failure) = primary_failure(eval) {
            root = failure;
            if let Err(outcome) = cancel_and_drain(eval, host, reactor, machine).await {
                return outcome;
            }
            break;
        }
        match drive_one_with_root(eval, host, reactor, Some(machine)).await {
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
    machine: &mut crate::eval::machine::EvalMachine,
) -> Result<(), EvaluationOutcome> {
    eval.scheduler
        .cancel_unsettled(etas_host::execution::CancellationReason::ChildFailed)
        .map_err(|error| scheduler_failure(eval, error))?;
    record_cancellation_events(eval);
    while eval.scheduler.has_unsettled_tasks() || !eval.cpu_jobs.is_empty() || reactor.has_pending()
    {
        match drive_one_with_root(eval, host, reactor, Some(machine)).await {
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
    machine: &mut crate::eval::machine::EvalMachine,
) -> Result<(), EvaluationOutcome> {
    if machine.has_input() {
        return Ok(());
    }
    loop {
        if machine
            .abandon_cancelled_agent_handler(eval)
            .map_err(|fault| {
                EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault))
            })?
        {
            return Ok(());
        }
        if machine.root_wait.is_none() {
            return Ok(());
        }
        if let Some(RootTaskWait::Host(wait)) = &machine.root_wait {
            if let Some(input) = wait.take_input() {
                machine.root_wait = None;
                machine.restore_task_input(input);
                return Ok(());
            }
        }
        if let Some(failure) = primary_failure(eval) {
            eval.scheduler
                .cancel_unsettled(etas_host::execution::CancellationReason::ChildFailed)
                .map_err(|error| scheduler_failure(eval, error))?;
            record_cancellation_events(eval);
            return Err(failure);
        }
        match drive_one_with_root(eval, host, reactor, Some(machine)).await {
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
    machine: &mut crate::eval::machine::EvalMachine,
) -> Result<Option<TaskOutcome>, EvaluationOutcome> {
    loop {
        if machine
            .abandon_cancelled_agent_handler(eval)
            .map_err(|fault| {
                EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault))
            })?
        {
            return Ok(None);
        }
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
            Ok(Some(outcome)) => return Ok(Some(outcome.clone())),
            Ok(None) => {}
            Err(error) => return Err(scheduler_failure(eval, error)),
        }
        match drive_one_with_root(eval, host, reactor, Some(machine)).await {
            Ok(true) => {
                if machine.root_wait.is_none() {
                    return Ok(None);
                }
            }
            Ok(false) => return Err(scheduler_deadlock(eval)),
            Err(outcome) => return Err(outcome),
        }
    }
}

/// Commit completed waits without executing user code. A ready parent must own
/// its input before a sibling can delegate a request to its lexical Handler.
fn drain_task_wakeups(eval: &mut EvalContext<'_>) -> Result<bool, EvaluationOutcome> {
    let mut progressed = false;
    while let Some(handle) = eval.scheduler.next_ready_wakeup() {
        progressed = true;
        let wakeup = eval
            .scheduler
            .take_wakeup(handle)
            .map_err(|error| scheduler_failure(eval, error))?;
        let mut task = eval.scheduler.take_ready_task(handle);
        if let Err(outcome) = resume_task_wakeup(eval, handle, &mut task, wakeup) {
            // The coordinator owns this machine lease; no user body is run.
            task.state = super::scheduler::TaskState::Running;
            finish_task(eval, handle, task, outcome)?;
        } else {
            eval.scheduler.restore_ready_task(handle, task);
        }
    }
    Ok(progressed)
}

fn resume_task_wakeup(
    eval: &mut EvalContext<'_>,
    handle: TaskHandle,
    task: &mut ScheduledTask,
    wakeup: Option<TaskWakeup>,
) -> Result<(), EvaluationOutcome> {
    if task
        .local
        .cancelled_agent_handler()
        .map_err(|error| scheduler_failure(eval, error.into()))?
        .is_some()
    {
        let (program, floor) = task.machine.program_scope().map_or_else(
            || {
                (
                    task.program
                        .clone()
                        .expect("Handler owner has a checked program"),
                    task.handler_floor,
                )
            },
            |scope| (scope.program, scope.handler_floor),
        );
        let mut child = eval.scheduled_task_context(&program, &mut task.local, handle.task, floor);
        if task
            .machine
            .abandon_cancelled_agent_handler(&mut child)
            .map_err(|fault| {
                EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault))
            })?
        {
            task.host_wait = None;
            task.waiting_continuation = None;
            return Ok(());
        }
    }
    match wakeup {
        Some(TaskWakeup::Completion { scope, event }) => {
            eval.events.push(WorkflowEvent::TaskReady {
                scope: scope.0,
                task: handle.task.0,
            });
            let Some(TaskResume::Completion {
                continuation,
                result_type,
                ..
            }) = task.waiting_continuation.take()
            else {
                return Err(task_fault(
                    eval,
                    "completion wakeup has no saved completion continuation",
                ));
            };
            let value =
                crate::value::task::source::completion_value(result_type, event, &eval.scheduler);
            task.machine
                .resume(ControlSignal::pending_continuation(PendingContinuation {
                    continuation,
                    input: ContinuationInput::Value(value),
                }));
        }
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
                    task.machine
                        .resume(signal_from_outcome(outcome, continuation)?);
                }
                TaskResume::Transfer {
                    token,
                    continuation,
                    output,
                    span,
                } => {
                    if let TaskOutcome::Completed(value) = &outcome {
                        validate_continuation_value(eval, token, value, output, span)?;
                    }
                    finish_parked_transfer(
                        &mut task.machine,
                        &mut task.local,
                        token,
                        continuation,
                        outcome,
                        span,
                    )?;
                }
                TaskResume::Run(_)
                | TaskResume::Handler { .. }
                | TaskResume::DeferredHandler { .. }
                | TaskResume::Completion { .. } => {
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
            task.machine
                .resume(signal_from_outcome(outcome, continuation)?);
        }
        None => {}
    }
    Ok(())
}

fn dispatch_ready(eval: &mut EvalContext<'_>) -> Result<Option<TaskHandle>, EvaluationOutcome> {
    if !eval.scheduler.has_source_policies() {
        return Ok(eval.scheduler.dispatch_ready());
    }
    loop {
        let Some(snapshot) = eval.scheduler.ready_snapshot() else {
            return Ok(None);
        };
        let index = if let Some(owner) = snapshot.policy_owner {
            let mut policy = eval.scheduler.take_source_policy(owner);
            let result = policy.select(eval, &snapshot.entries);
            eval.scheduler.restore_source_policy(owner, policy);
            match result {
                Ok(index) => index,
                Err(ControlSignal::Fault(fault)) => {
                    eval.diagnostics.push(fault.clone().into_diagnostic());
                    return Err(EvaluationOutcome::Failed(
                        crate::api::RunFailure::ExecutionFault(*fault),
                    ));
                }
                Err(ControlSignal::Cancelled(cause)) => {
                    return Err(EvaluationOutcome::Cancelled(cause));
                }
                Err(_) => {
                    return Err(task_fault(
                        eval,
                        "source scheduling policy has invalid control",
                    ));
                }
            }
        } else {
            0
        };
        let selected = snapshot.entries[index].handle;
        if let Some(handle) = eval
            .scheduler
            .dispatch_selected(selected)
            .map_err(|error| scheduler_failure(eval, error))?
        {
            return Ok(Some(handle));
        }
    }
}

async fn drive_one_with_root<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    reactor: &mut TaskHostReactor<'host>,
    mut root: Option<&mut crate::eval::machine::EvalMachine>,
) -> Result<bool, EvaluationOutcome> {
    record_cancellation_events(eval);
    let terminal_events = eval.scheduler.terminal_event_count();
    eval.scheduler
        .reap_closed_agents()
        .map_err(|error| scheduler_failure(eval, error))?;
    eval.scheduler
        .wake_cancelled_agent_handlers()
        .map_err(|error| scheduler_failure(eval, error))?;
    eval.scheduler
        .finish_available_host_drains()
        .map_err(|error| scheduler_failure(eval, error))?;
    for (task, execution) in eval.scheduler.take_resource_drains() {
        reactor.drain_resources(task, execution);
    }
    if drain_cpu_completions(eval)? {
        return Ok(true);
    }
    if let Some(completed) = reactor.poll_ready().await {
        settle_host_reply(eval, completed)?;
        return Ok(true);
    }
    if drain_task_wakeups(eval)? {
        return Ok(true);
    }
    eval.scheduler
        .wake_exposed_handlers()
        .map_err(|error| scheduler_failure(eval, error))?;
    let ready = dispatch_ready(eval)?;
    record_resource_events(eval);
    let Some(handle) = ready else {
        // Reaping an owned Agent or finishing a Host drain may settle a
        // wait without enqueuing another runnable body.
        if eval.scheduler.terminal_event_count() > terminal_events {
            record_cancellation_events(eval);
            return Ok(true);
        }
        if !eval.cpu_jobs.is_empty() {
            wait_for_cpu_or_host(eval, reactor).await?;
            return Ok(true);
        }
        let Some(completed) = reactor.wait_next().await else {
            return Ok(false);
        };
        settle_host_reply(eval, completed)?;
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

    if let Err(outcome) = resume_task_wakeup(eval, handle, &mut task, wakeup) {
        finish_task(eval, handle, task, outcome)?;
        return Ok(true);
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
            job: dispatch_sequence,
            dispatch_sequence,
        });
        eval.events.push(WorkflowEvent::TaskWaitingCpu {
            scope: handle.owner.0,
            task: handle.task.0,
            job: dispatch_sequence,
        });
        return Ok(true);
    }

    'poll: loop {
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
            crate::host::validate_host_readiness(child.plan, host.availability(), span)
        } else {
            crate::host::validate_execution_support(child.plan, span)
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
                finish_task(eval, handle, task, EvaluationOutcome::Completed(value))?;
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

        if let PendingBoundary::Checkpoint { label, span } = boundary {
            drop(child);
            eval.scheduler
                .restore_running(handle, task)
                .and_then(|()| eval.scheduler.requeue(handle))
                .map_err(|error| scheduler_failure(eval, error))?;
            let root = root.expect("scheduled checkpoint has a root coordinator machine");
            super::checkpoint::create(eval, reactor, root, label, span).await?;
            return Ok(true);
        }
        if let PendingBoundary::Task(control) = boundary {
            match *control {
                PendingTaskControl::NativeAuthorization(pending) => {
                    match dispatch::authorization::advance(&mut child, host, *pending) {
                        dispatch::authorization::AuthorizationStart::Complete(signal) => {
                            task.machine.resume(signal);
                            continue;
                        }
                        dispatch::authorization::AuthorizationStart::Pending(pending) => {
                            let operation = pending.operation_id();
                            drop(child);
                            task.host_wait =
                                Some(reactor.start_authorization(HostOwner::Task(handle), pending));
                            eval.scheduler
                                .restore_running(handle, task)
                                .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                                .map_err(|error| scheduler_failure(eval, error))?;
                            return Ok(true);
                        }
                    }
                }
                PendingTaskControl::AuthorizedNative(pending) => {
                    match Box::pin(dispatch::authorization::dispatch(
                        &mut child,
                        host,
                        &mut task.machine,
                        *pending,
                    ))
                    .await
                    {
                        dispatch::authorization::AuthorizedDispatchStart::Signal(signal) => {
                            task.machine.resume(signal);
                            continue;
                        }
                        dispatch::authorization::AuthorizedDispatchStart::Ready => continue,
                        dispatch::authorization::AuthorizedDispatchStart::Failed => {
                            let outcome = evaluation_failure(&mut child);
                            drop(child);
                            finish_task(eval, handle, task, outcome)?;
                            return Ok(true);
                        }
                        dispatch::authorization::AuthorizedDispatchStart::Pending(pending) => {
                            drop(child);
                            let wait = reactor.start_authorized(HostOwner::Task(handle), pending);
                            let operation = wait.operation_id();
                            task.host_wait = Some(wait);
                            eval.scheduler
                                .restore_running(handle, task)
                                .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                                .map_err(|error| scheduler_failure(eval, error))?;
                            return Ok(true);
                        }
                    }
                }
                control @ (PendingTaskControl::ManagedAgent(_)
                | PendingTaskControl::ManagedAuthorization(_)) => {
                    match start_managed(&mut child, host, control) {
                        dispatch::managed_agent::ManagedDispatchStart::Complete(signal) => {
                            task.machine.resume(signal);
                            continue;
                        }
                        dispatch::managed_agent::ManagedDispatchStart::Pending(pending) => {
                            let operation = pending.operation_id();
                            drop(child);
                            task.host_wait =
                                Some(reactor.start_managed(HostOwner::Task(handle), pending));
                            eval.scheduler
                                .restore_running(handle, task)
                                .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                                .map_err(|error| scheduler_failure(eval, error))?;
                            return Ok(true);
                        }
                    }
                }
                PendingTaskControl::AgentReply {
                    task: target,
                    continuation,
                    ..
                } => {
                    task.waiting_continuation = Some(TaskResume::Await(continuation));
                    drop(child);
                    eval.scheduler
                        .restore_running(handle, task)
                        .map_err(|error| scheduler_failure(eval, error))?;
                    if let AwaitDecision::Ready(outcome) = eval
                        .scheduler
                        .await_task(handle, target)
                        .map_err(|error| scheduler_failure(eval, error))?
                    {
                        eval.scheduler
                            .wake_task_wait(
                                handle,
                                TaskWakeup::Task {
                                    completed: target,
                                    outcome,
                                },
                            )
                            .map_err(|error| scheduler_failure(eval, error))?;
                    }
                    return Ok(true);
                }
                PendingTaskControl::NextCompletion {
                    scope,
                    result_type,
                    span,
                } => {
                    task.waiting_continuation = Some(TaskResume::Completion {
                        continuation: Continuation::BlockValue,
                        result_type,
                        span,
                    });
                    drop(child);
                    eval.scheduler
                        .restore_running(handle, task)
                        .map_err(|error| scheduler_failure(eval, error))?;
                    match eval
                        .scheduler
                        .await_completion(handle, scope)
                        .map_err(|error| scheduler_failure(eval, error))?
                    {
                        CompletionDecision::Parked => return Ok(true),
                        CompletionDecision::Ready(event) => {
                            task = eval
                                .scheduler
                                .take_running(handle)
                                .map_err(|error| scheduler_failure(eval, error))?;
                            let Some(TaskResume::Completion {
                                continuation,
                                result_type,
                                ..
                            }) = task.waiting_continuation.take()
                            else {
                                return Err(task_fault(
                                    eval,
                                    "completion delivery lost its continuation",
                                ));
                            };
                            let value = crate::value::task::source::completion_value(
                                result_type,
                                event,
                                &eval.scheduler,
                            );
                            task.machine.resume(ControlSignal::pending_continuation(
                                PendingContinuation {
                                    continuation,
                                    input: ContinuationInput::Value(value),
                                },
                            ));
                            continue;
                        }
                    }
                }
                control @ (PendingTaskControl::ResumeHandled { .. }
                | PendingTaskControl::RestoreHandlerWait { .. }) => {
                    drop(child);
                    if let PendingTaskControl::ResumeHandled {
                        task: target,
                        handler,
                        value,
                        ..
                    } = control
                    {
                        eval.scheduler
                            .resume_handled_action(target, handler, value)
                            .map_err(|error| scheduler_failure(eval, error))?;
                    }
                    match task
                        .machine
                        .root_wait
                        .take()
                        .expect("scheduled caller restores its wait")
                    {
                        RootTaskWait::Completion {
                            scope,
                            continuation,
                            result_type,
                            span,
                        } => {
                            task.waiting_continuation = Some(TaskResume::Completion {
                                continuation,
                                result_type,
                                span,
                            });
                            eval.scheduler
                                .restore_running(handle, task)
                                .map_err(|error| scheduler_failure(eval, error))?;
                            if let CompletionDecision::Ready(event) = eval
                                .scheduler
                                .await_completion(handle, scope)
                                .map_err(|error| scheduler_failure(eval, error))?
                            {
                                eval.scheduler
                                    .wake_task_wait(handle, TaskWakeup::Completion { scope, event })
                                    .map_err(|error| scheduler_failure(eval, error))?;
                            }
                            return Ok(true);
                        }
                        RootTaskWait::Ready(input) => {
                            task.machine.restore_task_input(*input);
                            continue;
                        }
                        RootTaskWait::Host(wait) => {
                            if let Some(input) = wait.take_input() {
                                task.machine.restore_task_input(input);
                                continue;
                            }
                            let operation = wait.operation_id();
                            task.host_wait = Some(wait);
                            eval.scheduler
                                .restore_running(handle, task)
                                .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                                .map_err(|error| scheduler_failure(eval, error))?;
                            return Ok(true);
                        }
                        RootTaskWait::Await {
                            target,
                            continuation,
                        } => {
                            task.waiting_continuation = Some(TaskResume::Await(continuation));
                            eval.scheduler
                                .restore_running(handle, task)
                                .map_err(|error| scheduler_failure(eval, error))?;
                            if let AwaitDecision::Ready(outcome) = eval
                                .scheduler
                                .await_task(handle, target)
                                .map_err(|error| scheduler_failure(eval, error))?
                            {
                                eval.scheduler
                                    .wake_task_wait(
                                        handle,
                                        TaskWakeup::Task {
                                            completed: target,
                                            outcome,
                                        },
                                    )
                                    .map_err(|error| scheduler_failure(eval, error))?;
                            }
                            return Ok(true);
                        }
                        RootTaskWait::Run { scope } => {
                            task.waiting_continuation =
                                Some(TaskResume::Run(Continuation::BlockValue));
                            eval.scheduler
                                .restore_running(handle, task)
                                .map_err(|error| scheduler_failure(eval, error))?;
                            if let ScopeDecision::Ready(outcome) = eval
                                .scheduler
                                .await_scope(handle, scope)
                                .map_err(|error| scheduler_failure(eval, error))?
                            {
                                eval.scheduler
                                    .wake_task_wait(handle, TaskWakeup::Scope { scope, outcome })
                                    .map_err(|error| scheduler_failure(eval, error))?;
                            }
                            return Ok(true);
                        }
                        RootTaskWait::Transfer {
                            target,
                            token,
                            continuation,
                            output,
                            span,
                        } => {
                            task.waiting_continuation = Some(TaskResume::Transfer {
                                token,
                                continuation,
                                output,
                                span,
                            });
                            eval.scheduler
                                .restore_running(handle, task)
                                .map_err(|error| scheduler_failure(eval, error))?;
                            if let AwaitDecision::Ready(outcome) = eval
                                .scheduler
                                .await_task(handle, target)
                                .map_err(|error| scheduler_failure(eval, error))?
                            {
                                eval.scheduler
                                    .wake_task_wait(
                                        handle,
                                        TaskWakeup::Task {
                                            completed: target,
                                            outcome,
                                        },
                                    )
                                    .map_err(|error| scheduler_failure(eval, error))?;
                            }
                            return Ok(true);
                        }
                        _ => unreachable!("delegated caller wait is a task, scope or ready input"),
                    }
                }
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
                    if let Err(outcome) = validate_continuation_value(
                        &mut child,
                        token,
                        &value,
                        consumed.output,
                        span,
                    ) {
                        drop(child);
                        finish_task(eval, handle, task, outcome)?;
                        return Ok(true);
                    }
                    task.machine
                        .transfer_from(child.task, token, consumed.continuation, value);
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
                    let continuation = match task.machine.park_transfer(
                        &mut child,
                        token,
                        consumed.continuation,
                        span,
                    ) {
                        Ok(continuation) => continuation,
                        Err(fault) => {
                            task.machine.resume(ControlSignal::Fault(Box::new(fault)));
                            continue;
                        }
                    };
                    task.waiting_continuation = Some(TaskResume::Transfer {
                        token,
                        continuation,
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
                            if let TaskOutcome::Completed(value) = &outcome {
                                if let Err(outcome) =
                                    validate_continuation_value(eval, token, value, output, span)
                                {
                                    finish_task(eval, handle, task, outcome)?;
                                    return Ok(true);
                                }
                                eval.events.push(WorkflowEvent::ContinuationTransferred {
                                    scope: token.owner.0,
                                    continuation: token.continuation.0,
                                    task: Some(handle.task.0),
                                    kind: "park".to_owned(),
                                });
                            }
                            if let Err(outcome) = finish_parked_transfer(
                                &mut task.machine,
                                &mut task.local,
                                token,
                                continuation,
                                outcome,
                                span,
                            ) {
                                finish_task(eval, handle, task, outcome)?;
                                return Ok(true);
                            }
                            continue;
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
            if perform.error_type.is_none() && child.task.inherited_handlers != 0 {
                let reserved = match task.waiting_continuation.take() {
                    Some(TaskResume::DeferredHandler { handler }) => Some(handler),
                    None => None,
                    Some(_) => unreachable!("performing task still owns a scheduler wait"),
                };
                let inherited = child
                    .task
                    .inherited_handler_records()
                    .iter()
                    .filter(|scope| reserved.is_none_or(|handler| handler == scope.id))
                    .cloned()
                    .collect::<Vec<_>>();
                let child_logical_identity = child.task.logical_identity;
                let child_host_context = child.task.capture_agent_host_context();
                let child_record_context = child.task.capture_record_context();
                drop(child);
                let mut owner = None;
                let mut parent = task.local.parent;
                while let Some(super::scheduler::TaskIdentity::Scheduled(id)) = parent {
                    parent = eval.scheduler.task_parent(id);
                    let Some((caller_handle, mut caller)) =
                        eval.scheduler.take_handler_caller(id, &inherited)
                    else {
                        continue;
                    };
                    let wait = match caller.state {
                        super::scheduler::TaskState::Waiting(
                            super::scheduler::TaskWait::Completion(scope),
                        ) => {
                            let Some(TaskResume::Completion {
                                continuation,
                                result_type,
                                span,
                            }) = caller.waiting_continuation.take()
                            else {
                                unreachable!("completion caller continuation")
                            };
                            RootTaskWait::Completion {
                                scope,
                                continuation,
                                result_type,
                                span,
                            }
                        }
                        super::scheduler::TaskState::Waiting(super::scheduler::TaskWait::Host(
                            _,
                        )) => RootTaskWait::Host(
                            caller.host_wait.take().expect("caller owns its Host wait"),
                        ),
                        super::scheduler::TaskState::Ready => RootTaskWait::Ready(Box::new(
                            caller
                                .machine
                                .take_task_input()
                                .expect("ready caller input"),
                        )),
                        super::scheduler::TaskState::Waiting(super::scheduler::TaskWait::Task(
                            target,
                        )) => {
                            let target = TaskHandle {
                                owner: if target == handle.task {
                                    handle.owner
                                } else {
                                    eval.scheduler.task_scope(target)
                                },
                                task: target,
                            };
                            match caller
                                .waiting_continuation
                                .take()
                                .expect("awaited caller continuation")
                            {
                                TaskResume::Await(continuation) => RootTaskWait::Await {
                                    target,
                                    continuation,
                                },
                                TaskResume::Transfer {
                                    token,
                                    continuation,
                                    output,
                                    span,
                                } => RootTaskWait::Transfer {
                                    target,
                                    token,
                                    continuation,
                                    output,
                                    span,
                                },
                                _ => unreachable!("task waiter continuation"),
                            }
                        }
                        super::scheduler::TaskState::Waiting(
                            super::scheduler::TaskWait::Scope(scope),
                        ) => {
                            let Some(TaskResume::Run(_)) = caller.waiting_continuation.take()
                            else {
                                unreachable!("scope caller continuation")
                            };
                            RootTaskWait::Run { scope }
                        }
                        _ => unreachable!("handler caller admission"),
                    };
                    caller.machine.root_wait = Some(wait);
                    let caller_scope = caller.machine.program_scope();
                    let caller_program = caller_scope
                        .as_ref()
                        .map(|scope| scope.program.clone())
                        .or_else(|| caller.program.clone())
                        .expect("caller checked program");
                    let caller_floor = caller_scope
                        .as_ref()
                        .map_or(caller.handler_floor, |scope| scope.handler_floor);
                    let held = if caller.state == super::scheduler::TaskState::Ready {
                        eval.scheduler.arm_owned_continuations(id)
                    } else {
                        Vec::new()
                    };
                    let mut caller_ctx = eval.scheduled_task_context(
                        &caller_program,
                        &mut caller.local,
                        caller_handle.task,
                        caller_floor,
                    );
                    let dormant = if held.is_empty() {
                        Ok(None)
                    } else {
                        caller.machine.dormant_task_handler(
                            &mut caller_ctx,
                            &task.machine,
                            &program,
                            &perform,
                            &inherited,
                            &held,
                        )
                    };
                    let relay = match dormant {
                        Ok(None) => caller.machine.relay_task_handler(
                            &mut caller_ctx,
                            handle,
                            child_logical_identity,
                            child_host_context.clone(),
                            child_record_context.clone(),
                            &task.machine,
                            &program,
                            &perform,
                            &inherited,
                        ),
                        Ok(Some(_)) => Ok(None),
                        Err(ref fault) => Err(fault.clone()),
                    };
                    let relay_fault = match relay {
                        Ok(handler) => {
                            owner = handler;
                            None
                        }
                        Err(fault) => Some(fault),
                    };
                    drop(caller_ctx);
                    if owner.is_none() {
                        match caller
                            .machine
                            .root_wait
                            .take()
                            .expect("unmatched caller retains its wait")
                        {
                            RootTaskWait::Completion {
                                continuation,
                                result_type,
                                span,
                                ..
                            } => {
                                caller.waiting_continuation = Some(TaskResume::Completion {
                                    continuation,
                                    result_type,
                                    span,
                                });
                            }
                            RootTaskWait::Ready(input) => caller.machine.restore_task_input(*input),
                            RootTaskWait::Host(wait) => caller.host_wait = Some(wait),
                            RootTaskWait::Await { continuation, .. } => {
                                caller.waiting_continuation = Some(TaskResume::Await(continuation))
                            }
                            RootTaskWait::Transfer {
                                token,
                                continuation,
                                output,
                                span,
                                ..
                            } => {
                                caller.waiting_continuation = Some(TaskResume::Transfer {
                                    token,
                                    continuation,
                                    output,
                                    span,
                                })
                            }
                            RootTaskWait::Run { .. } => {
                                caller.waiting_continuation =
                                    Some(TaskResume::Run(Continuation::BlockValue))
                            }
                            _ => unreachable!(),
                        }
                    }
                    if owner.is_some()
                        && caller.state
                            == super::scheduler::TaskState::Waiting(
                                super::scheduler::TaskWait::Task(handle.task),
                            )
                    {
                        task.waiters.retain(|waiter| !matches!(waiter, super::scheduler::TaskWaiter::Task(id) if *id == caller_handle.task));
                    }
                    eval.scheduler.restore_handler_caller(
                        caller_handle,
                        caller,
                        owner.is_some(),
                        handle.task,
                    );
                    if let Some(fault) = relay_fault {
                        task.machine.resume(ControlSignal::Fault(Box::new(fault)));
                        continue 'poll;
                    }
                    if let Ok(Some(handler)) = dormant {
                        // The checked arm owns this request. Its action arguments and
                        // continuation stay with the child until the owner exposes it.
                        let mut perform = perform;
                        perform.expr = None;
                        perform.forwarded_call = None;
                        perform.default_boundary = None;
                        task.machine.resume(ControlSignal::Perform(perform));
                        task.waiting_continuation = Some(TaskResume::DeferredHandler { handler });
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_handler_owner(handle, id))
                            .map_err(|error| scheduler_failure(eval, error))?;
                        return Ok(true);
                    }
                    if owner.is_some() {
                        break;
                    }
                }
                if owner.is_none() {
                    owner = match root.as_deref_mut() {
                        Some(root) => match root.relay_task_handler(
                            eval,
                            handle,
                            child_logical_identity,
                            child_host_context.clone(),
                            child_record_context.clone(),
                            &task.machine,
                            &program,
                            &perform,
                            &inherited,
                        ) {
                            Ok(handler) => handler,
                            Err(fault) => {
                                task.machine.resume(ControlSignal::Fault(Box::new(fault)));
                                continue 'poll;
                            }
                        },
                        None => None,
                    };
                }
                if let Some(handler) = owner {
                    task.waiting_continuation = Some(TaskResume::Handler {
                        continuation: perform.continuation,
                        handler,
                    });
                    eval.scheduler
                        .restore_running(handle, task)
                        .and_then(|()| eval.scheduler.wait_for_handler(handle, handler))
                        .map_err(|error| scheduler_failure(eval, error))?;
                    return Ok(true);
                }
                if reserved.is_some() {
                    return Err(task_fault(
                        eval,
                        "selected lexical Handler ended with a child request outstanding",
                    ));
                }
                child = eval.scheduled_task_context(
                    &program,
                    &mut task.local,
                    handle.task,
                    handler_floor,
                );
            }
            if perform.default_boundary.is_some() {
                let signal = child.execute_checked_default_action(*perform);
                task.machine.resume(signal);
                continue;
            }
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
            if let Some(signal) = child.source_host_action(&perform) {
                task.machine.resume(signal);
                continue;
            }
            if child.source_compiler_default(&perform, etas_std::CompilerActionRole::TaskYield) {
                let signal = child.resume_perform_signal(*perform, InterpValue::Unit);
                task.machine.resume(signal);
                drop(child);
                eval.scheduler
                    .restore_running(handle, task)
                    .and_then(|()| eval.scheduler.requeue(handle))
                    .map_err(|error| scheduler_failure(eval, error))?;
                tokio::task::yield_now().await;
                return Ok(true);
            }
            if let Some(target) = awaited_task(&child, &perform) {
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
                                let wait = reactor.start_approval(HostOwner::Task(handle), pending);
                                assert_eq!(wait.operation_id(), operation);
                                task.host_wait = Some(wait);
                                eval.scheduler
                                    .restore_running(handle, task)
                                    .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                                    .map_err(|error| scheduler_failure(eval, error))?;
                                return Ok(true);
                            }
                        }
                    }
                    dispatch::compiler::CompilerDispatchStart::Pending(pending) => {
                        let operation = pending.operation_id();
                        drop(child);
                        let wait = reactor.start_compiler(HostOwner::Task(handle), pending);
                        assert_eq!(wait.operation_id(), operation);
                        task.host_wait = Some(wait);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
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
                        let wait = reactor.start_model(HostOwner::Task(handle), pending);
                        assert_eq!(wait.operation_id(), operation);
                        task.host_wait = Some(wait);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
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
                        let wait = reactor.start_tool(HostOwner::Task(handle), pending);
                        assert_eq!(wait.operation_id(), operation);
                        task.host_wait = Some(wait);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
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
                        let wait = reactor.start_memory(HostOwner::Task(handle), pending);
                        assert_eq!(wait.operation_id(), operation);
                        task.host_wait = Some(wait);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
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
                        let wait = reactor.start_console(HostOwner::Task(handle), pending);
                        assert_eq!(wait.operation_id(), operation);
                        task.host_wait = Some(wait);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
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
                        let wait = reactor.start_command(HostOwner::Task(handle), pending);
                        assert_eq!(wait.operation_id(), operation);
                        task.host_wait = Some(wait);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
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
                        let wait = reactor.start_host(HostOwner::Task(handle), pending);
                        assert_eq!(wait.operation_id(), operation);
                        task.host_wait = Some(wait);
                        eval.scheduler
                            .restore_running(handle, task)
                            .and_then(|()| eval.scheduler.wait_for_host(handle, operation))
                            .map_err(|error| scheduler_failure(eval, error))?;
                        return Ok(true);
                    }
                }
            }
            PendingBoundary::Perform(_) => unreachable!("perform handled above"),
            PendingBoundary::Task(_) => unreachable!("task control handled above"),
            PendingBoundary::Checkpoint { .. } => unreachable!("checkpoint handled above"),
        };
        task.machine.resume(signal);
    }
}

/// Settle a native reply in its issuing program, independently of the owner's
/// current Handler arm. The saved wait holds its input until resumed/captured.
pub(super) fn settle_host_reply(
    eval: &mut EvalContext<'_>,
    ready: ReadyTaskHost,
) -> Result<(), EvaluationOutcome> {
    let (owner, operation, completed) = match ready {
        ReadyTaskHost::Cleanup { task, report } => {
            let report = report.map_err(|error| scheduler_failure(eval, error.into()))?;
            eval.scheduler
                .finish_resource_drain(task, report)
                .map_err(|error| scheduler_failure(eval, error))?;
            record_cancellation_events(eval);
            return Ok(());
        }
        ReadyTaskHost::Operation {
            owner,
            operation,
            completed,
        } => (owner, operation, completed),
    };
    let stage = |ctx: &mut EvalContext<'_>| {
        operation.context.run(ctx, |ctx| {
            let mut input = crate::eval::machine::EvalMachine::new();
            let diagnostics = ctx.diagnostics.len();
            let result = commit_host_completion(ctx, &mut input, completed);
            if !operation.is_waiting() {
                // The continuation was abandoned. Accounting, write evidence and
                // trace still settle; its reply cannot fail the continuing arm.
                ctx.diagnostics.truncate(diagnostics);
                return Ok(());
            }
            result?;
            operation.complete_input(
                input
                    .take_task_input()
                    .expect("Host completion has an input"),
            );
            Ok(())
        })
    };
    match owner {
        HostOwner::Root => stage(eval),
        HostOwner::Task(handle) => {
            let mut task = eval.scheduler.take_host_owner(handle);
            let program = task
                .program
                .clone()
                .expect("Host owner has a checked program");
            let mut child = eval.scheduled_task_context(
                &program,
                &mut task.local,
                handle.task,
                task.handler_floor,
            );
            let result = stage(&mut child);
            drop(child);
            let active = task
                .host_wait
                .as_ref()
                .is_some_and(|wait| wait.operation_id() == operation.id());
            if active && result.is_ok() {
                let wait = task.host_wait.take().expect("active Host wait");
                task.machine
                    .restore_task_input(wait.take_input().expect("settled Host input"));
            }
            eval.scheduler.restore_host_owner(handle, task);
            result?;
            if eval
                .scheduler
                .finish_host_drain(handle)
                .map_err(|error| scheduler_failure(eval, error))?
            {
                record_cancellation_events(eval);
                return Ok(());
            }
            if active {
                wake_from_host(eval, handle, operation.id())?;
            }
            Ok(())
        }
    }
}

/// Commit Host results on the coordinator without polling language code. The
/// checkpoint barrier uses the same commit path as normal task execution.
pub(super) fn commit_host_completion(
    eval: &mut EvalContext<'_>,
    machine: &mut crate::eval::machine::EvalMachine,
    completed: CompletedTaskHost,
) -> Result<(), EvaluationOutcome> {
    let signal = match completed {
        CompletedTaskHost::Authorization(completed) => {
            Some(dispatch::authorization::finish(eval, completed))
        }
        CompletedTaskHost::Managed(completed) => {
            Some(dispatch::managed_agent::finish(eval, completed))
        }
        CompletedTaskHost::Console(completed) => dispatch::console::finish(eval, completed),
        CompletedTaskHost::Command(completed) => {
            dispatch::command::finish(eval, machine, completed)
        }
        CompletedTaskHost::Host(completed) => dispatch::host::finish(eval, machine, completed),
        CompletedTaskHost::Tool(completed) => {
            dispatch::tool::finish(eval, machine, completed);
            return Ok(());
        }
        CompletedTaskHost::Model(completed) => {
            dispatch::model::finish(eval, machine, completed);
            return Ok(());
        }
        CompletedTaskHost::Approval(completed) => dispatch::approval::finish(eval, completed),
        CompletedTaskHost::Compiler(completed) => Some(dispatch::compiler::finish(eval, completed)),
        CompletedTaskHost::Memory(completed) => dispatch::memory::finish(eval, machine, completed),
    };
    let signal = signal.ok_or_else(|| evaluation_failure(eval))?;
    machine.resume(signal);
    Ok(())
}

pub(super) async fn wait_for_cpu_or_host(
    eval: &mut EvalContext<'_>,
    reactor: &mut TaskHostReactor<'_>,
) -> Result<(), EvaluationOutcome> {
    let executor = eval.cpu_executor.clone();
    tokio::select! {
        _ = executor.completion_signal().notified() => {},
        completed = reactor.wait_next(), if reactor.has_pending() => {
            if let Some(completed) = completed {
                settle_host_reply(eval, completed)?;
            }
        }
    }
    Ok(())
}

pub(super) fn drain_cpu_completions(eval: &mut EvalContext<'_>) -> Result<bool, EvaluationOutcome> {
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
            job: completion.metrics.dispatch_sequence,
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
            job: completion.metrics.dispatch_sequence,
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
            record_cancellation_events(eval);
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
                EvaluationOutcome::Completed(Box::new(value))
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

fn finish_parked_transfer(
    machine: &mut crate::eval::machine::EvalMachine,
    local: &mut crate::eval::TaskLocalState,
    token: crate::value::ContinuationTokenValue,
    parked: crate::eval::machine::ParkedContinuation,
    outcome: TaskOutcome,
    span: etas_core::Span,
) -> Result<(), EvaluationOutcome> {
    let Some(projections) = parked.projections else {
        machine.transfer_signal(
            local,
            token,
            signal_from_outcome(outcome, parked.continuation)?,
        );
        return Ok(());
    };
    let outcome = match outcome {
        TaskOutcome::Completed(mut value) => {
            for (responder, projection) in projections {
                value = projection.resume(value, &responder).map_err(|reason| {
                    let fault = crate::control::ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        reason,
                    );
                    EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault))
                })?;
            }
            TaskOutcome::Completed(value)
        }
        outcome => outcome,
    };
    machine.resume(signal_from_outcome(outcome, parked.continuation)?);
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
    eval.scheduler
        .restore_running(handle, task)
        .map_err(|error| scheduler_failure(eval, error))?;
    let result = match outcome {
        EvaluationOutcome::Completed(value) => eval.scheduler.complete(handle, *value),
        EvaluationOutcome::Failed(failure) => eval.scheduler.fail(handle, failure),
        EvaluationOutcome::Cancelled(cause) => eval.scheduler.cancelled(handle, cause),
    };
    result
        .and_then(|()| eval.scheduler.apply_group_policy(handle))
        .map_err(|error| scheduler_failure(eval, error))?;
    record_cancellation_events(eval);
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
    let accounts = eval
        .task
        .host_budget()
        .account_snapshots()
        .expect("task scope budget observation succeeds");
    let budget = &accounts[0];
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

pub(super) fn record_cancellation_events(eval: &mut EvalContext<'_>) {
    for event in eval.scheduler.take_agent_events() {
        eval.events.push(event);
    }
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
    for (handle, state) in eval.take_terminal_task_events() {
        let outcome = match state {
            TaskState::Completed => "completed",
            TaskState::Raised => "raised",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
            _ => unreachable!("terminal event has a terminal task state"),
        };
        eval.events.push(WorkflowEvent::TaskSettled {
            scope: handle.owner.0,
            task: handle.task.0,
            outcome: outcome.to_owned(),
        });
        eval.events.push(WorkflowEvent::TaskCleanupCompleted {
            scope: handle.owner.0,
            task: handle.task.0,
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
        Ok(Some(TaskOutcome::Raised(perform))) => {
            Some(EvaluationOutcome::Failed(crate::api::RunFailure::Language(
                Box::new(crate::diagnostics::unhandled_effect_action(perform)),
            )))
        }
        _ => None,
    }
}

fn scheduler_deadlock(eval: &mut EvalContext<'_>) -> EvaluationOutcome {
    if let Some(message) = eval.scheduler.agent_wait_cycle() {
        let span = crate::diagnostics::item_span(eval.checked, eval.entry_item);
        let fault = crate::control::ExecutionFault::new(
            AnalysisDiagnosticCode::AgentRequestDeadlock,
            span,
            message,
        );
        eval.diagnostics.push(fault.clone().into_diagnostic());
        return EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault));
    }
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
    token: crate::value::ContinuationTokenValue,
    value: &InterpValue,
    expected: etas_types::TypeId,
    span: etas_core::Span,
) -> Result<(), EvaluationOutcome> {
    let program = eval
        .scheduler
        .continuation_program(token)
        .map_err(|error| scheduler_failure(eval, error))?;
    let checked = program
        .as_ref()
        .map_or(eval.checked, |program| &program.checked);
    let plan = program.as_ref().map_or(eval.plan, |program| &program.plan);
    crate::value::contract::runtime::validate_value(
        checked,
        plan,
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

pub(super) fn task_fault(
    eval: &mut EvalContext<'_>,
    message: impl Into<String>,
) -> EvaluationOutcome {
    let span = crate::diagnostics::item_span(eval.checked, eval.entry_item);
    let fault = crate::control::ExecutionFault::new(
        AnalysisDiagnosticCode::MissingCheckedFact,
        span,
        message,
    );
    eval.diagnostics.push(fault.clone().into_diagnostic());
    EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault))
}

use etas_core::{AnalysisDiagnosticCode, Diagnostic};

use crate::{
    control::ControlSignal,
    eval::{
        EvalContext,
        machine::{EvalMachine, MachinePoll, PendingBoundary},
    },
    host::HostServices,
    value::InterpValue,
};

use super::{dispatch, scheduler::HostOwner};

pub(super) fn evaluation_failure(eval: &mut EvalContext<'_>) -> EvaluationOutcome {
    if let Some(diagnostic) = eval
        .diagnostics
        .iter()
        .rev()
        .find(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
    {
        return EvaluationOutcome::Failed(crate::api::RunFailure::Language(Box::new(
            diagnostic.clone(),
        )));
    }
    let span = crate::diagnostics::item_span(eval.checked, eval.entry_item);
    match eval.cancellation_signal(span) {
        Some(ControlSignal::Cancelled(cause)) => return EvaluationOutcome::Cancelled(cause),
        Some(ControlSignal::Fault(fault)) => {
            eval.diagnostics.push(fault.clone().into_diagnostic());
            return EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(*fault));
        }
        _ => {}
    }
    let fault = crate::control::ExecutionFault::new(
        AnalysisDiagnosticCode::MissingCheckedFact,
        span,
        "evaluation stopped without a failure diagnostic or cancellation cause",
    );
    eval.diagnostics.push(fault.clone().into_diagnostic());
    EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault))
}

#[derive(Debug)]
pub(crate) enum EvaluationOutcome {
    Completed(Box<InterpValue>),
    Failed(crate::api::RunFailure),
    Cancelled(etas_host::execution::CancellationCause),
}

impl EvaluationOutcome {
    pub(crate) fn into_outcome(self) -> crate::api::RunOutcome {
        match self {
            Self::Completed(value) => crate::api::RunOutcome::Completed(*value),
            Self::Failed(failure) => crate::api::RunOutcome::Failed(failure),
            Self::Cancelled(cause) => crate::api::RunOutcome::Cancelled(cause),
        }
    }
}

pub async fn execute_entry(
    eval: &mut EvalContext<'_>,
    signal: ControlSignal,
    host: &dyn HostServices,
) -> EvaluationOutcome {
    let machine =
        EvalMachine::for_entry(eval.checked_execution_program(), eval.entry_args.to_vec());
    Box::pin(drive_eval_signal(eval, signal, host, machine)).await
}

pub async fn execute_entry_from_snapshot(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    checkpoint: &crate::orchestration::InterpreterCheckpoint,
    revisions: Option<std::sync::Arc<crate::api::code::PreparedRevisionSet>>,
    ordering: crate::eval::machine::snapshot::ordering::OrderedRestoreChecks,
) -> EvaluationOutcome {
    let snapshot = &checkpoint.machine;
    if snapshot.total_call_frames().map_or(true, |count| {
        count > eval.execution_limits.max_call_depth.get()
    }) {
        let span = crate::diagnostics::item_span(eval.checked, eval.entry_item);
        eval.diagnostics.push(Diagnostic::analysis(
            AnalysisDiagnosticCode::MissingCheckedFact,
            span,
            "checkpoint active and suspended calls exceed current execution limits",
        ));
        return evaluation_failure(eval);
    }
    let signal = eval.execute_from_checkpoint_signal(checkpoint, revisions.as_deref());
    if let ControlSignal::Fault(fault) = signal {
        eval.diagnostics.push(fault.clone().into_diagnostic());
        return EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(*fault));
    }
    let restored = if let Some(tasks) = checkpoint.tasks.as_deref() {
        eval.restore_task_checkpoint(
            tasks,
            snapshot,
            crate::eval::machine::snapshot::CheckpointRoot {
                program: eval.checked_execution_program(),
                args: eval.entry_args.to_vec(),
            },
            revisions.clone(),
            Some(ordering.clone()),
        )
    } else {
        EvalMachine::from_snapshot_with_resources(
            snapshot,
            eval.checked,
            eval.plan.slots.clone(),
            &eval.plan.dispatch,
            &eval.plan.closures,
            &eval.host_context,
            crate::eval::machine::snapshot::SnapshotRestoreResources {
                task: None,
                locked_model: eval.locked_model.clone(),
                limits: &eval.storage_limits,
                revisions,
                root: Some(crate::eval::machine::snapshot::CheckpointRoot {
                    program: eval.checked_execution_program(),
                    args: eval.entry_args.to_vec(),
                }),
                ordering: Some(ordering.clone()),
                residual: Some(crate::eval::machine::snapshot::ResidualRestoreInput {
                    prepared: eval.plan.residual.clone(),
                    log: eval.task.residual.log.clone(),
                    limits: eval.task.residual.limits,
                }),
            },
        )
    };
    let machine = match restored {
        Ok(machine) => machine,
        Err(message) => {
            let span = crate::diagnostics::item_span(eval.checked, eval.entry_item);
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                format!("checkpoint machine stack is invalid: {message}"),
            ));
            let outcome = evaluation_failure(eval);
            return super::residual_exit::finish(eval, None, outcome);
        }
    };
    let residual = &checkpoint.machine.residual;
    eval.task.residual.restore_counters((
        residual.next_cursor,
        residual.next_attempt,
        residual.next_invocation,
    ));
    if let Err(message) = eval.seed_resume_counters(&checkpoint.trace) {
        let span = crate::diagnostics::item_span(eval.checked, eval.entry_item);
        eval.diagnostics
            .push(crate::diagnostics::missing_checked_fact(span, message));
        let outcome = evaluation_failure(eval);
        return super::residual_exit::finish(eval, Some(machine), outcome);
    }
    if let Err(outcome) = super::restore_ordering::validate(eval, host, &ordering).await {
        return super::residual_exit::finish(eval, Some(machine), outcome);
    }
    Box::pin(drive_eval_signal(eval, signal, host, machine)).await
}

async fn drive_eval_signal(
    eval: &mut EvalContext<'_>,
    mut signal: ControlSignal,
    host: &dyn HostServices,
    mut machine: EvalMachine,
) -> EvaluationOutcome {
    let mut machine_has_input = machine.has_input();
    let mut task_hosts = super::scheduler::TaskHostReactor::new();
    if let Some(outcome) = super::task::take_root_settlement(&mut machine) {
        let outcome = super::task::settle(eval, host, &mut task_hosts, outcome, &mut machine).await;
        return super::residual_exit::finish(eval, Some(machine), outcome);
    }
    loop {
        let scope = machine.program_scope();
        let result = if let Some(scope) = scope {
            let mut scoped = eval.scoped_context(&scope.program, scope.handler_floor);
            // A scoped call inherits dynamic handlers from its caller. Its
            // isolated default-handler summary cannot preflight Host services;
            // only an action that escapes those handlers reaches Host dispatch.
            let span = crate::diagnostics::item_span(scoped.checked, scope.program.plan.entry.item);
            let readiness = if scope.handler_floor == 0 {
                crate::host::validate_host_readiness(&scope.program.plan, host.availability(), span)
            } else {
                crate::host::validate_execution_support(&scope.program.plan, span)
            };
            if !readiness.is_empty() {
                scoped.diagnostics.extend(readiness);
                ScopeOutcome::Finished(evaluation_failure(&mut scoped))
            } else {
                drive_scope(
                    &mut scoped,
                    &mut signal,
                    host,
                    &mut task_hosts,
                    &mut machine,
                    &mut machine_has_input,
                )
                .await
            }
        } else {
            drive_scope(
                eval,
                &mut signal,
                host,
                &mut task_hosts,
                &mut machine,
                &mut machine_has_input,
            )
            .await
        };
        if let ScopeOutcome::Finished(outcome) = result {
            let outcome =
                super::task::settle(eval, host, &mut task_hosts, outcome, &mut machine).await;
            return super::residual_exit::finish(eval, Some(machine), outcome);
        }
    }
}

enum ScopeOutcome {
    Finished(EvaluationOutcome),
    ProgramChanged,
}

async fn drive_scope<'host>(
    eval: &mut EvalContext<'_>,
    signal: &mut ControlSignal,
    host: &'host dyn HostServices,
    task_hosts: &mut super::scheduler::TaskHostReactor<'host>,
    machine: &mut EvalMachine,
    machine_has_input: &mut bool,
) -> ScopeOutcome {
    if machine.root_wait.is_some() {
        if let Err(outcome) = super::task::resume_root_wait(eval, host, task_hosts, machine).await {
            return ScopeOutcome::Finished(outcome);
        }
        *machine_has_input = true;
    }
    loop {
        if !*machine_has_input {
            machine.resume(std::mem::replace(
                signal,
                ControlSignal::Value(InterpValue::Unit),
            ));
        }
        *machine_has_input = false;
        let boundary = match machine.run_until_yield(eval) {
            MachinePoll::ProgramChanged => {
                *machine_has_input = true;
                return ScopeOutcome::ProgramChanged;
            }
            MachinePoll::Complete(value) => {
                return ScopeOutcome::Finished(EvaluationOutcome::Completed(value));
            }
            MachinePoll::Cancelled(cause) => {
                return ScopeOutcome::Finished(EvaluationOutcome::Cancelled(cause));
            }
            MachinePoll::CooperativeYield => {
                tokio::task::yield_now().await;
                *machine_has_input = true;
                continue;
            }
            MachinePoll::Yield(boundary) => boundary,
            MachinePoll::Fault(fault) => {
                eval.diagnostics.push(fault.clone().into_diagnostic());
                return ScopeOutcome::Finished(EvaluationOutcome::Failed(
                    crate::api::RunFailure::ExecutionFault(fault),
                ));
            }
        };
        if let Some(diagnostic) =
            crate::host::validate_boundary_readiness(&boundary, host.availability())
        {
            eval.diagnostics.push(diagnostic);
            return ScopeOutcome::Finished(evaluation_failure(eval));
        }
        *signal = match boundary {
            PendingBoundary::Checkpoint { label, span } => {
                if let Err(outcome) =
                    super::checkpoint::create(eval, task_hosts, machine, label, span).await
                {
                    return ScopeOutcome::Finished(outcome);
                }
                *machine_has_input = true;
                continue;
            }
            PendingBoundary::Model(model) => {
                match dispatch::model::begin(eval, host, *model, machine).await {
                    dispatch::model::ModelDispatchStart::Complete(true) => {
                        *machine_has_input = true;
                        continue;
                    }
                    dispatch::model::ModelDispatchStart::Complete(false) => {
                        return ScopeOutcome::Finished(evaluation_failure(eval));
                    }
                    dispatch::model::ModelDispatchStart::Pending(pending) => {
                        let operation = pending.operation_id();
                        let registered = task_hosts.start_model(HostOwner::Root, pending);
                        assert_eq!(
                            registered, operation,
                            "registered host operation changed id"
                        );
                        if let Err(outcome) =
                            super::task::await_root_host(eval, host, task_hosts, machine).await
                        {
                            return ScopeOutcome::Finished(outcome);
                        }
                        *machine_has_input = true;
                        continue;
                    }
                }
            }
            PendingBoundary::Tool(tool) => {
                match dispatch::tool::begin(eval, host, *tool, machine).await {
                    dispatch::tool::ToolDispatchStart::Complete(true) => {
                        *machine_has_input = true;
                        continue;
                    }
                    dispatch::tool::ToolDispatchStart::Complete(false) => {
                        return ScopeOutcome::Finished(evaluation_failure(eval));
                    }
                    dispatch::tool::ToolDispatchStart::Pending(pending) => {
                        let operation = pending.operation_id();
                        let registered = task_hosts.start_tool(HostOwner::Root, pending);
                        assert_eq!(
                            registered, operation,
                            "registered host operation changed id"
                        );
                        if let Err(outcome) =
                            super::task::await_root_host(eval, host, task_hosts, machine).await
                        {
                            return ScopeOutcome::Finished(outcome);
                        }
                        *machine_has_input = true;
                        continue;
                    }
                }
            }
            PendingBoundary::Perform(perform) => {
                // The committed residual permit is revalidated and consumed
                // immediately before the governed default dispatch.
                if let Err(fault) = eval.consume_residual_permit(&perform) {
                    eval.diagnostics.push(fault.clone().into_diagnostic());
                    return ScopeOutcome::Finished(EvaluationOutcome::Failed(
                        crate::api::RunFailure::ExecutionFault(fault),
                    ));
                }
                if let Some(target) = super::task::awaited_task(&perform) {
                    match super::task::await_from_root(
                        eval,
                        host,
                        task_hosts,
                        machine,
                        target,
                        perform.continuation,
                    )
                    .await
                    {
                        Ok(()) => {
                            *machine_has_input = true;
                            continue;
                        }
                        Err(outcome) => return ScopeOutcome::Finished(outcome),
                    }
                } else if let Some(signal) = eval.default_agent_inference(&perform) {
                    signal
                } else {
                    match dispatch::compiler::begin(eval, host, *perform).await {
                        dispatch::compiler::CompilerDispatchStart::Complete(signal) => signal,
                        dispatch::compiler::CompilerDispatchStart::NotCompiler(perform) => {
                            match dispatch::approval::begin(eval, host, *perform, machine) {
                                dispatch::approval::ApprovalDispatchStart::Complete(Some(
                                    signal,
                                )) => signal,
                                dispatch::approval::ApprovalDispatchStart::Complete(None) => {
                                    return ScopeOutcome::Finished(evaluation_failure(eval));
                                }
                                dispatch::approval::ApprovalDispatchStart::Pending(pending) => {
                                    let operation = pending.operation_id();
                                    let registered =
                                        task_hosts.start_approval(HostOwner::Root, pending);
                                    assert_eq!(
                                        registered, operation,
                                        "registered host operation changed id"
                                    );
                                    if let Err(outcome) = super::task::await_root_host(
                                        eval, host, task_hosts, machine,
                                    )
                                    .await
                                    {
                                        return ScopeOutcome::Finished(outcome);
                                    }
                                    *machine_has_input = true;
                                    continue;
                                }
                            }
                        }
                        dispatch::compiler::CompilerDispatchStart::Pending(pending) => {
                            let operation = pending.operation_id();
                            let registered = task_hosts.start_compiler(HostOwner::Root, pending);
                            assert_eq!(
                                registered, operation,
                                "registered host operation changed id"
                            );
                            if let Err(outcome) =
                                super::task::await_root_host(eval, host, task_hosts, machine).await
                            {
                                return ScopeOutcome::Finished(outcome);
                            }
                            *machine_has_input = true;
                            continue;
                        }
                    }
                }
            }
            PendingBoundary::Memory(memory) => {
                let Some(signal) = dispatch::memory::dispatch(eval, host, *memory, machine).await
                else {
                    return ScopeOutcome::Finished(evaluation_failure(eval));
                };
                signal
            }
            PendingBoundary::Session(session) => {
                dispatch::session::dispatch(eval, host, *session).await
            }
            PendingBoundary::Console(console) => {
                match dispatch::console::begin(eval, host, *console).await {
                    dispatch::console::ConsoleDispatchStart::Complete(Some(signal)) => signal,
                    dispatch::console::ConsoleDispatchStart::Complete(None) => {
                        return ScopeOutcome::Finished(evaluation_failure(eval));
                    }
                    dispatch::console::ConsoleDispatchStart::Pending(pending) => {
                        let operation = pending.operation_id();
                        let registered = task_hosts.start_console(HostOwner::Root, pending);
                        assert_eq!(
                            registered, operation,
                            "registered host operation changed id"
                        );
                        if let Err(outcome) =
                            super::task::await_root_host(eval, host, task_hosts, machine).await
                        {
                            return ScopeOutcome::Finished(outcome);
                        }
                        *machine_has_input = true;
                        continue;
                    }
                }
            }
            PendingBoundary::Command(command) => {
                match dispatch::command::begin(eval, host, *command, machine).await {
                    dispatch::command::CommandDispatchStart::Complete(Some(signal)) => signal,
                    dispatch::command::CommandDispatchStart::Complete(None) => {
                        return ScopeOutcome::Finished(evaluation_failure(eval));
                    }
                    dispatch::command::CommandDispatchStart::Pending(pending) => {
                        let operation = pending.operation_id();
                        let registered = task_hosts.start_command(HostOwner::Root, pending);
                        assert_eq!(
                            registered, operation,
                            "registered host operation changed id"
                        );
                        if let Err(outcome) =
                            super::task::await_root_host(eval, host, task_hosts, machine).await
                        {
                            return ScopeOutcome::Finished(outcome);
                        }
                        *machine_has_input = true;
                        continue;
                    }
                }
            }
            PendingBoundary::Host(boundary) => {
                match dispatch::host::begin(eval, host, *boundary, machine).await {
                    dispatch::host::HostBoundaryDispatchStart::Complete(Some(signal)) => signal,
                    dispatch::host::HostBoundaryDispatchStart::Complete(None) => {
                        return ScopeOutcome::Finished(evaluation_failure(eval));
                    }
                    dispatch::host::HostBoundaryDispatchStart::Pending(pending) => {
                        let operation = pending.operation_id();
                        let registered = task_hosts.start_host(HostOwner::Root, pending);
                        assert_eq!(
                            registered, operation,
                            "registered host operation changed id"
                        );
                        if let Err(outcome) =
                            super::task::await_root_host(eval, host, task_hosts, machine).await
                        {
                            return ScopeOutcome::Finished(outcome);
                        }
                        *machine_has_input = true;
                        continue;
                    }
                }
            }
            PendingBoundary::Task(control) => {
                match super::task::control_from_root(eval, host, task_hosts, machine, *control)
                    .await
                {
                    Ok(()) => {
                        *machine_has_input = true;
                        continue;
                    }
                    Err(outcome) => return ScopeOutcome::Finished(outcome),
                }
            }
        };
    }
}

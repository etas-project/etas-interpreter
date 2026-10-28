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

use super::dispatch;

fn evaluation_failure(eval: &mut EvalContext<'_>) -> EvaluationOutcome {
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

pub(crate) enum EvaluationOutcome {
    Completed(InterpValue),
    Failed(crate::api::RunFailure),
    Cancelled(etas_host::execution::CancellationCause),
}

impl EvaluationOutcome {
    pub(crate) fn into_outcome(self) -> crate::api::RunOutcome {
        match self {
            Self::Completed(value) => crate::api::RunOutcome::Completed(value),
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
    drive_eval_signal(eval, signal, host, EvalMachine::new()).await
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
    let machine = match EvalMachine::from_snapshot_with_resources(
        snapshot,
        eval.checked,
        eval.plan.slots.clone(),
        &eval.plan.dispatch,
        &eval.plan.closures,
        &eval.host_context,
        crate::eval::machine::snapshot::SnapshotRestoreResources {
            locked_model: eval.locked_model.clone(),
            limits: &eval.storage_limits,
            revisions,
            root: Some(crate::eval::machine::snapshot::CheckpointRoot {
                program: eval.checked_execution_program(),
                args: eval.entry_args.to_vec(),
            }),
            ordering: Some(ordering.clone()),
        },
    ) {
        Ok(machine) => machine,
        Err(message) => {
            let span = crate::diagnostics::item_span(eval.checked, eval.entry_item);
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                format!("checkpoint machine stack is invalid: {message}"),
            ));
            return evaluation_failure(eval);
        }
    };
    if let Err(message) = eval.seed_resume_counters(&checkpoint.trace) {
        let span = crate::diagnostics::item_span(eval.checked, eval.entry_item);
        eval.diagnostics
            .push(crate::diagnostics::missing_checked_fact(span, message));
        return evaluation_failure(eval);
    }
    if let Err(outcome) = super::restore_ordering::validate(eval, host, &ordering).await {
        return outcome;
    }
    let signal = eval.execute_from_checkpoint_signal(checkpoint);
    drive_eval_signal(eval, signal, host, machine).await
}

async fn drive_eval_signal(
    eval: &mut EvalContext<'_>,
    mut signal: ControlSignal,
    host: &dyn HostServices,
    mut machine: EvalMachine,
) -> EvaluationOutcome {
    let mut machine_has_input = false;
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
                return evaluation_failure(&mut scoped);
            }
            drive_scope(
                &mut scoped,
                &mut signal,
                host,
                &mut machine,
                &mut machine_has_input,
            )
            .await
        } else {
            drive_scope(
                eval,
                &mut signal,
                host,
                &mut machine,
                &mut machine_has_input,
            )
            .await
        };
        if let ScopeOutcome::Finished(outcome) = result {
            return outcome;
        }
    }
}

enum ScopeOutcome {
    Finished(EvaluationOutcome),
    ProgramChanged,
}

async fn drive_scope(
    eval: &mut EvalContext<'_>,
    signal: &mut ControlSignal,
    host: &dyn HostServices,
    machine: &mut EvalMachine,
    machine_has_input: &mut bool,
) -> ScopeOutcome {
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
                return ScopeOutcome::Finished(EvaluationOutcome::Completed(*value));
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
            PendingBoundary::Model(model) => {
                if !dispatch::model::dispatch(eval, host, &model, machine).await {
                    return ScopeOutcome::Finished(evaluation_failure(eval));
                }
                *machine_has_input = true;
                continue;
            }
            PendingBoundary::Tool(tool) => {
                if !dispatch::tool::dispatch(eval, host, *tool, machine).await {
                    return ScopeOutcome::Finished(evaluation_failure(eval));
                }
                *machine_has_input = true;
                continue;
            }
            PendingBoundary::Perform(perform) => {
                if let Some(signal) = eval.default_agent_inference(&perform) {
                    signal
                } else {
                    match dispatch::compiler::dispatch(eval, host, *perform).await {
                        Ok(signal) => signal,
                        Err(perform) => {
                            let Some(signal) =
                                dispatch::approval::dispatch(eval, host, *perform, machine).await
                            else {
                                return ScopeOutcome::Finished(evaluation_failure(eval));
                            };
                            signal
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
                let Some(signal) = dispatch::console::dispatch(eval, host, *console).await else {
                    return ScopeOutcome::Finished(evaluation_failure(eval));
                };
                signal
            }
            PendingBoundary::Command(command) => {
                let Some(signal) = dispatch::command::dispatch(eval, host, *command, machine).await
                else {
                    return ScopeOutcome::Finished(evaluation_failure(eval));
                };
                signal
            }
            PendingBoundary::Host(boundary) => {
                let Some(signal) = dispatch::host::dispatch(eval, host, *boundary, machine).await
                else {
                    return ScopeOutcome::Finished(evaluation_failure(eval));
                };
                signal
            }
        };
    }
}

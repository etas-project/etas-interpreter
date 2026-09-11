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
    signal: ControlSignal,
    host: &dyn HostServices,
    snapshot: &crate::orchestration::MachineSnapshot,
) -> EvaluationOutcome {
    let machine = match EvalMachine::from_snapshot(
        snapshot,
        eval.checked,
        eval.plan.slots.clone(),
        &eval.plan.dispatch,
        &eval.host_context,
        &eval.storage_limits,
    ) {
        Ok(machine) => machine,
        Err(message) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::MissingCheckedFact,
                crate::diagnostics::item_span(eval.checked, eval.entry_item),
                format!("checkpoint machine stack is invalid: {message}"),
            ));
            return evaluation_failure(eval);
        }
    };
    drive_eval_signal(eval, signal, host, machine).await
}

pub(super) async fn drive_eval_signal(
    eval: &mut EvalContext<'_>,
    mut signal: ControlSignal,
    host: &dyn HostServices,
    mut machine: EvalMachine,
) -> EvaluationOutcome {
    let mut machine_has_input = false;
    loop {
        if !machine_has_input {
            machine.resume(std::mem::replace(
                &mut signal,
                ControlSignal::Value(InterpValue::Unit),
            ));
        }
        machine_has_input = false;
        let boundary = match machine.run_until_yield(eval) {
            MachinePoll::Complete(value) => return EvaluationOutcome::Completed(*value),
            MachinePoll::Cancelled(cause) => return EvaluationOutcome::Cancelled(cause),
            MachinePoll::CooperativeYield => {
                tokio::task::yield_now().await;
                machine_has_input = true;
                continue;
            }
            MachinePoll::Yield(boundary) => boundary,
            MachinePoll::Fault(fault) => {
                eval.diagnostics.push(fault.clone().into_diagnostic());
                return EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault));
            }
        };
        signal = match boundary {
            PendingBoundary::Model(model) => {
                if !dispatch::model::dispatch(eval, host, &model, &mut machine).await {
                    return evaluation_failure(eval);
                }
                machine_has_input = true;
                continue;
            }
            PendingBoundary::Tool(tool) => {
                if !dispatch::tool::dispatch(eval, host, *tool, &mut machine).await {
                    return evaluation_failure(eval);
                }
                machine_has_input = true;
                continue;
            }
            PendingBoundary::Perform(perform) => {
                let Some(signal) =
                    dispatch::approval::dispatch(eval, host, *perform, &mut machine).await
                else {
                    return evaluation_failure(eval);
                };
                signal
            }
            PendingBoundary::Memory(memory) => {
                let Some(signal) =
                    dispatch::memory::dispatch(eval, host, *memory, &mut machine).await
                else {
                    return evaluation_failure(eval);
                };
                signal
            }
            PendingBoundary::Session(session) => {
                dispatch::session::dispatch(eval, host, *session).await
            }
            PendingBoundary::Console(console) => {
                let Some(signal) = dispatch::console::dispatch(eval, host, *console).await else {
                    return evaluation_failure(eval);
                };
                signal
            }
            PendingBoundary::Command(command) => {
                let Some(signal) =
                    dispatch::command::dispatch(eval, host, *command, &mut machine).await
                else {
                    return evaluation_failure(eval);
                };
                signal
            }
            PendingBoundary::Host(boundary) => {
                let Some(signal) =
                    dispatch::host::dispatch(eval, host, *boundary, &mut machine).await
                else {
                    return evaluation_failure(eval);
                };
                signal
            }
        };
    }
}

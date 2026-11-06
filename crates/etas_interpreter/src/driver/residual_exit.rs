use super::drive::EvaluationOutcome;
use crate::{
    control::residual::ResidualExitKind,
    eval::{EvalContext, machine::EvalMachine},
};

/// Settle all live scope owners before dropping the machine, then include
/// exits of abandoned continuations in the report. Never replace an existing
/// cancellation or execution failure with a secondary obligation failure.
pub(super) fn finish(
    eval: &mut EvalContext<'_>,
    machine: Option<EvalMachine>,
    outcome: EvaluationOutcome,
) -> EvaluationOutcome {
    let kind = match &outcome {
        EvaluationOutcome::Cancelled(_) => ResidualExitKind::Cancellation,
        EvaluationOutcome::Failed(_) => ResidualExitKind::Fault,
        // Normal scope frames must already have validated and released their
        // scopes. Any still-active captured scope was abandoned, not returned.
        EvaluationOutcome::Completed(_) => ResidualExitKind::Abort,
    };
    let log = eval.task.residual.log.clone();
    let faults = log.finish_active(kind);
    drop(machine);
    let mut diagnostics = log.take_diagnostics();
    diagnostics.extend(faults.into_iter().map(|fault| fault.into_diagnostic()));
    let outcome = if let EvaluationOutcome::Completed(_) = &outcome {
        match diagnostics.first() {
            Some(diagnostic) => EvaluationOutcome::Failed(crate::api::RunFailure::Language(
                Box::new(diagnostic.clone()),
            )),
            None => outcome,
        }
    } else {
        outcome
    };
    eval.diagnostics.extend(diagnostics);
    outcome
}

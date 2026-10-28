use super::drive::EvaluationOutcome;
use crate::{
    control::{Continuation, ControlSignal, PendingCall},
    eval::{
        EvalContext,
        machine::snapshot::ordering::{OrderedRestoreChecks, RestoredOrderedValue},
    },
    host::HostServices,
    value::{InterpValue, NumericValue},
};

pub(super) async fn validate(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    checks: &OrderedRestoreChecks,
) -> Result<(), EvaluationOutcome> {
    let checks = checks.take().map_err(|message| invalid(eval, message))?;
    for check in checks {
        let mut scoped = eval.scoped_context(&check.program, 0);
        let (target, count) = match &check.value {
            RestoredOrderedValue::Map(value) => (value.comparator(), value.len()),
            RestoredOrderedValue::Set(value) => (value.comparator(), value.len()),
        };
        let target = target
            .cloned()
            .ok_or_else(|| invalid(&mut scoped, "ordered restore check has no comparator"))?;
        crate::eval::collection_ordering::validate_comparator(scoped.checked, scoped.plan, &target)
            .map_err(|message| invalid(&mut scoped, message))?;
        let key = |index| match &check.value {
            RestoredOrderedValue::Map(value) => value.entry(index).map(|entry| entry.0.clone()),
            RestoredOrderedValue::Set(value) => value.key(index).cloned(),
        };
        let span = crate::diagnostics::item_span(scoped.checked, scoped.entry_item);
        for index in 1..count {
            let left = key(index - 1)
                .ok_or_else(|| invalid(&mut scoped, "ordered restore lost its left key"))?;
            let right = key(index)
                .ok_or_else(|| invalid(&mut scoped, "ordered restore lost its right key"))?;
            let signal = ControlSignal::pending_call(PendingCall {
                purity: crate::control::CallPurity::Pure,
                site: None,
                target: target.clone(),
                args: vec![left, right],
                span,
                continuation: Continuation::BlockValue,
            });
            match super::drive::execute_entry(&mut scoped, signal, host).await {
                EvaluationOutcome::Completed(InterpValue::Number(NumericValue::I32(order)))
                    if order < 0 => {}
                EvaluationOutcome::Completed(InterpValue::Number(NumericValue::I32(_))) => {
                    return Err(invalid(
                        &mut scoped,
                        "restored ordered keys are duplicate or out-of-order under their checked comparison",
                    ));
                }
                EvaluationOutcome::Completed(_) => {
                    return Err(invalid(
                        &mut scoped,
                        "restored key comparison did not return i32",
                    ));
                }
                outcome => return Err(outcome),
            }
        }
    }
    Ok(())
}

fn invalid(eval: &mut EvalContext<'_>, message: impl Into<String>) -> EvaluationOutcome {
    let origin = crate::diagnostics::item_span(eval.checked, eval.entry_item);
    eval.diagnostics
        .push(crate::diagnostics::invalid_arguments(origin, message));
    EvaluationOutcome::Failed(crate::api::RunFailure::RestoreRejected { origin })
}

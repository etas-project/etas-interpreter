use super::*;
use crate::control::ExecutionFault;

impl<'a> EvalContext<'a> {
    pub(super) fn eval_try_expr(
        &mut self,
        try_expr: HirExprId,
        operand: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        if !self.checked.effects.try_captures.contains_key(&try_expr) {
            return missing_try_capture_fault(span);
        }

        let continuation = Continuation::TryExpr {
            expr: try_expr,
            span,
        };
        match self.eval_expr(operand, frame) {
            ControlSignal::Value(value) => ControlSignal::Value(result_ok(value)),
            ControlSignal::Perform(perform) if self.can_capture_error_raise(try_expr, &perform) => {
                self.capture_error_raise(try_expr, &perform, span)
            }
            signal if is_pending_host_boundary_signal(&signal) => {
                compose_signal_continuation(signal, continuation)
            }
            other => other,
        }
    }

    pub(super) fn resume_try_expr(
        &mut self,
        try_expr: HirExprId,
        value: InterpValue,
        span: Span,
    ) -> ControlSignal {
        if self.checked.effects.try_captures.contains_key(&try_expr) {
            ControlSignal::Value(result_ok(value))
        } else {
            missing_try_capture_fault(span)
        }
    }

    pub(super) fn capture_error_raise(
        &mut self,
        try_expr: HirExprId,
        perform: &PendingPerform,
        span: Span,
    ) -> ControlSignal {
        let Some(capture) = self.checked.effects.try_captures.get(&try_expr) else {
            return missing_try_capture_fault(span);
        };
        let Some(source_error) = perform.error_type else {
            return ControlSignal::missing_checked_fact(
                "`?` captured Error.raise without a checked source error type",
                span,
            );
        };
        let error = match perform.args.as_slice() {
            [error] => error.clone(),
            _ => {
                return ControlSignal::invalid_arguments(
                    "Error.raise captured by `?` must carry exactly one error value",
                    span,
                );
            }
        };
        let error = if self.type_ids_match(source_error, capture.captured_error) {
            error
        } else {
            let Some(conversion) = capture.conversions.iter().find(|conversion| {
                self.type_ids_match(conversion.source_error, source_error)
                    && self.type_ids_match(conversion.target_error, capture.captured_error)
            }) else {
                return ControlSignal::missing_checked_fact(
                    "`?` captured an Error.raise without a checked conversion to the target error type",
                    span,
                );
            };
            match self.apply_checked_error_conversion(conversion, error, span) {
                Ok(converted) => converted,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            }
        };
        ControlSignal::Value(result_err(error))
    }

    pub(super) fn can_capture_error_raise(
        &self,
        try_expr: HirExprId,
        perform: &PendingPerform,
    ) -> bool {
        let Some(capture) = self.checked.effects.try_captures.get(&try_expr) else {
            return false;
        };
        perform.action.action == "raise"
            && perform
                .error_type
                .is_some_and(|error| self.can_capture_error_type(error, capture))
            && perform
                .action
                .effect
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.name == "Error")
    }

    fn can_capture_error_type(
        &self,
        error: etas_types::TypeId,
        capture: &etas_effects::TryCaptureFact,
    ) -> bool {
        self.type_ids_match(error, capture.captured_error)
            || capture.conversions.iter().any(|conversion| {
                self.type_ids_match(conversion.source_error, error)
                    && self.type_ids_match(conversion.target_error, capture.captured_error)
            })
    }

    fn apply_checked_error_conversion(
        &mut self,
        conversion: &etas_effects::ErrorConversionFact,
        error: InterpValue,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        if self.type_ids_match(conversion.source_error, conversion.target_error) {
            return Ok(error);
        }
        Err(ExecutionFault::new(
            AnalysisDiagnosticCode::UnhandledRuntimeError,
            span,
            "checked error conversion for `?` is not executable without a runtime conversion value",
        ))
    }

    fn type_ids_match(&self, left: etas_types::TypeId, right: etas_types::TypeId) -> bool {
        if left == right {
            return true;
        }
        match (
            self.checked.type_store.get(left),
            self.checked.type_store.get(right),
        ) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        }
    }
}

fn missing_try_capture_fault(span: Span) -> ControlSignal {
    ControlSignal::missing_checked_fact("postfix `?` requires a checked TryCaptureFact", span)
}

fn result_ok(value: InterpValue) -> InterpValue {
    InterpValue::Variant {
        name: "Ok".to_owned(),
        fields: vec![value],
    }
}

fn result_err(error: InterpValue) -> InterpValue {
    InterpValue::Variant {
        name: "Err".to_owned(),
        fields: vec![error],
    }
}

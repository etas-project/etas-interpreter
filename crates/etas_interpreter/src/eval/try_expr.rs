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
        let Some(capture) = self.checked.effects.try_captures.get(&try_expr) else {
            return missing_try_capture_fault(span);
        };
        let result_type = match self.materialized_type(
            capture.result_type,
            &frame.bindings_at(self.checked, etas_hir::HirOwner::Expr(try_expr)),
            span,
        ) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };

        let continuation = Continuation::TryExpr {
            expr: try_expr,
            result_type,
            span,
        };
        match self.eval_expr(operand, frame) {
            ControlSignal::Value(value) => {
                self.checked_result_value(result_type, "Ok", value, span)
            }
            ControlSignal::Perform(perform)
                if self.can_capture_error_raise(try_expr, result_type, &perform) =>
            {
                self.capture_error_raise(try_expr, result_type, &perform, span)
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
        result_type: etas_types::TypeId,
        value: InterpValue,
        span: Span,
    ) -> ControlSignal {
        if self.checked.effects.try_captures.contains_key(&try_expr) {
            self.checked_result_value(result_type, "Ok", value, span)
        } else {
            missing_try_capture_fault(span)
        }
    }

    pub(super) fn capture_error_raise(
        &mut self,
        try_expr: HirExprId,
        result_type: etas_types::TypeId,
        perform: &PendingPerform,
        span: Span,
    ) -> ControlSignal {
        let Some(capture) = self.checked.effects.try_captures.get(&try_expr) else {
            return missing_try_capture_fault(span);
        };
        let Some((_, captured_error)) = etas_types::value_representation::result_parameters(
            &self.checked.type_store,
            &self.checked.types.known_std_types,
            result_type,
        ) else {
            return ControlSignal::missing_checked_fact(
                "`?` requires its materialized checked Result type",
                span,
            );
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
        let error = if self.type_ids_match(source_error, captured_error) {
            error
        } else {
            let Some(conversion) = capture.conversions.iter().find(|conversion| {
                self.type_ids_match(conversion.source_error, source_error)
                    && self.type_ids_match(conversion.target_error, captured_error)
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
        self.checked_result_value(result_type, "Err", error, span)
    }

    pub(super) fn can_capture_error_raise(
        &self,
        try_expr: HirExprId,
        result_type: etas_types::TypeId,
        perform: &PendingPerform,
    ) -> bool {
        let Some(capture) = self.checked.effects.try_captures.get(&try_expr) else {
            return false;
        };
        let Some((_, captured_error)) = etas_types::value_representation::result_parameters(
            &self.checked.type_store,
            &self.checked.types.known_std_types,
            result_type,
        ) else {
            return false;
        };
        perform.action.action == "raise"
            && perform
                .error_type
                .is_some_and(|error| self.can_capture_error_type(error, captured_error, capture))
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
        captured_error: etas_types::TypeId,
        capture: &etas_effects::TryCaptureFact,
    ) -> bool {
        self.type_ids_match(error, captured_error)
            || capture.conversions.iter().any(|conversion| {
                self.type_ids_match(conversion.source_error, error)
                    && self.type_ids_match(conversion.target_error, captured_error)
            })
    }

    pub(super) fn checked_result_value(
        &self,
        result_type: etas_types::TypeId,
        name: &str,
        value: InterpValue,
        span: Span,
    ) -> ControlSignal {
        if matches!(
            self.checked.type_store.get(result_type),
            Some(etas_types::Type::Result { .. })
        ) {
            return ControlSignal::Value(InterpValue::Variant {
                name: name.into(),
                fields: vec![value].into(),
            });
        }
        match crate::value::enum_value::construct(result_type, name, vec![value], self.checked) {
            Ok(value) => ControlSignal::Value(value),
            Err(message) => ControlSignal::missing_checked_fact(message, span),
        }
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

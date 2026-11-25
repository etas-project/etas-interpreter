use super::*;
use crate::control::ExecutionFault;

impl EvalContext<'_> {
    pub(super) fn primitive_constructor_kind(
        &self,
        expr: HirExprId,
        bindings: &crate::control::GenericBindings,
        span: Span,
    ) -> Result<Option<etas_types::PrimitiveType>, ExecutionFault> {
        let Some(target) = self.plan.records.primitive_constructor(expr) else {
            return Ok(None);
        };
        let target = self.materialized_type(target, bindings, span)?;
        match self.checked.type_store.get(target) {
            Some(etas_types::Type::Primitive(kind)) => Ok(Some(*kind)),
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "primitive representation constructor has no concrete primitive type argument",
            )),
        }
    }

    pub(super) fn materialized_expression_type(
        &self,
        expr: HirExprId,
        bindings: &crate::control::GenericBindings,
        span: Span,
    ) -> Result<etas_types::TypeId, ExecutionFault> {
        let ty = self
            .checked
            .types
            .expression_producer_type(expr)
            .ok_or_else(|| {
                ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    "expression has no checked result type",
                )
            })?;
        self.materialized_type(ty, bindings, span)
    }

    pub(super) fn materialized_type(
        &self,
        ty: etas_types::TypeId,
        bindings: &crate::control::GenericBindings,
        span: Span,
    ) -> Result<etas_types::TypeId, ExecutionFault> {
        bindings
            .indexed_substitution(&self.checked.type_store, self.plan.type_lookup())
            .substitute(ty)
            .map_err(|error| {
                ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    format!("checked result type is not materialized: {error}"),
                )
            })
    }
}

use super::*;
use crate::control::ExecutionFault;

impl EvalContext<'_> {
    pub(super) fn materialized_expression_type(
        &self,
        expr: HirExprId,
        bindings: &crate::control::GenericBindings,
        span: Span,
    ) -> Result<etas_types::TypeId, ExecutionFault> {
        let ty = *self.checked.types.expr_types.get(&expr).ok_or_else(|| {
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
            .substitution(&self.checked.type_store)
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

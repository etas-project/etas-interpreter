use etas_core::Span;
use etas_hir::{HirArg, HirExpr, HirExprId, ResolveResult};

use crate::{control::ControlSignal, eval::EvalContext, value::InterpValue};

impl EvalContext<'_> {
    pub(in crate::eval) fn eval_tool_reference(
        &self,
        expression: HirExprId,
        span: Span,
    ) -> ControlSignal {
        let result = self.project_tool_reference(expression);
        match result {
            Ok(value) => ControlSignal::Value(value),
            Err(error) => ControlSignal::missing_checked_fact(error, span),
        }
    }

    fn project_tool_reference(&self, expression: HirExprId) -> Result<InterpValue, String> {
        let fact = self
            .checked
            .types
            .std_intrinsic_calls
            .get(&expression)
            .filter(|fact| fact.intrinsic == etas_std::intrinsic::runtime::TOOL_REFERENCE)
            .ok_or("tool reference has no checked intrinsic selection")?;
        let Some(HirExpr::Call { args, .. } | HirExpr::MethodCall { args, .. }) =
            self.checked.hir.exprs.get(expression)
        else {
            return Err("tool reference has no checked call expression".into());
        };
        let [HirArg::Positional(value) | HirArg::Named { value, .. }] = args.as_slice() else {
            return Err("tool reference has no single declaration operand".into());
        };
        let Some(HirExpr::Path(path)) = self.checked.hir.exprs.get(*value) else {
            return Err("tool reference operand is not a declaration path".into());
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return Err("tool reference declaration is unresolved".into());
        };
        let (identity, revision) = self.canonical_tool_identity(symbol)?;
        let projector = super::project::Projector {
            store: &self.checked.type_store,
            known: &self.checked.types.known_std_types.code,
        };
        projector.record(
            fact.signature.output,
            vec![
                ("id", InterpValue::String(identity.into())),
                ("revision", InterpValue::String(revision.into())),
            ],
        )
    }

    pub(in crate::eval) fn canonical_tool_identity(
        &self,
        symbol: etas_hir::SymbolId,
    ) -> Result<(String, String), String> {
        let prepared = self
            .code_definitions
            .as_ref()
            .filter(|prepared| prepared.is_for(self.checked))
            .ok_or("tool reference requires prepared canonical definition origins")?;
        let definitions = prepared.definitions();
        let (identity, revision) = definitions
            .catalog()
            .tool_reference_for_symbol(symbol, definitions.syntax_operations())
            .map_err(|error| error.to_string())?;
        Ok((
            identity.fingerprint().to_string(),
            revision.key().fingerprint().to_string(),
        ))
    }
}

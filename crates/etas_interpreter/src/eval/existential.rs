use super::*;

impl EvalContext<'_> {
    pub(super) fn pack_existential(
        &self,
        expr: HirExprId,
        value: InterpValue,
        bindings: &crate::control::GenericBindings,
    ) -> ControlSignal {
        let span = self.checked.hir.exprs[expr].span(&self.checked.hir.blocks);
        let Some(fact) = self.checked.types.existential_packs.get(&expr) else {
            return ControlSignal::missing_checked_fact(
                "existential value has no checked packing fact",
                span,
            );
        };
        let object_type = match self.materialized_type(fact.object_type, bindings, span) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let source_type = match self.materialized_type(fact.source_type, bindings, span) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        ControlSignal::Value(InterpValue::Existential(Box::new(
            crate::value::ExistentialValue {
                pack: expr,
                object_type,
                concrete_type: source_type,
                bindings: bindings.clone().shared(),
                value: value.into(),
            },
        )))
    }
}

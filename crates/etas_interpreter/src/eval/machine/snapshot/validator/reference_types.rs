use etas_hir::SymbolId;
use etas_types::{SymbolTypeFact, TypeId};

use super::{CallTargetSnapshot, SnapshotValidator, ValueSnapshot};

impl SnapshotValidator<'_> {
    pub(super) fn typed_expression_value(
        &self,
        value: &ValueSnapshot,
        expr: etas_hir::HirExprId,
        bindings: &crate::control::GenericBindings,
    ) -> Result<(), String> {
        self.snapshot_value_in(value, super::call_target::ReferenceScope::CheckedSlot)?;
        self.reference_expression(value, expr, bindings)
    }

    pub(super) fn reference_expression(
        &self,
        value: &ValueSnapshot,
        expr: etas_hir::HirExprId,
        bindings: &crate::control::GenericBindings,
    ) -> Result<(), String> {
        let ty = *self
            .checked
            .types
            .expr_types
            .get(&expr)
            .ok_or("checkpoint expression has no checked value type")?;
        self.reference_value(value, ty, bindings)
    }

    pub(super) fn pending_call_target(
        &self,
        call: etas_hir::HirExprId,
        target: &CallTargetSnapshot,
        site: &etas_hir::HirExpr,
        bindings: &crate::control::GenericBindings,
    ) -> Result<(), String> {
        let expected = crate::control::CheckedCallBindings::resolve(self.checked, call, bindings)?;
        if !expected.types.is_empty() || !expected.rows.is_empty() {
            let CallTargetSnapshot::Specialized {
                type_bindings,
                effect_row_bindings,
                ..
            } = target
            else {
                return Err("checkpoint call is missing its checked specialization".into());
            };
            if type_bindings != &expected.types || effect_row_bindings != &expected.rows {
                return Err(
                    "checkpoint call specialization differs from its checked generic arguments"
                        .into(),
                );
            }
        }
        if let etas_hir::HirExpr::Call { callee, .. } = site
            && !self.checked.types.agent_method_calls.contains_key(&call)
        {
            let mut specialization = bindings.clone();
            specialization.extend(expected.types);
            specialization.rows.extend(expected.rows);
            if self.pending_direct_intrinsic(call, target, site, &specialization)? {
                return Ok(());
            }
            let ty = *self
                .checked
                .types
                .expr_types
                .get(callee)
                .ok_or("checkpoint pending call has no checked callee type")?;
            return self.reference_target_type_with_bindings(target, ty, &specialization);
        }
        if !self.reference_presence.borrow_mut().contains(target) {
            return Ok(());
        }
        let mut seen = std::collections::HashSet::new();
        let mut pending = vec![target];
        while let Some(target) = pending.pop() {
            match target {
                CallTargetSnapshot::AgentRevision(reference) => {
                    let agent = self
                        .revisions
                        .ok_or("checkpoint agent reference requires freshly prepared revisions")?
                        .restore(reference, self.checked)?;
                    if let Some(symbol) = self.checked.types.agent_method_calls.get(&call) {
                        let selected = self
                            .checked
                            .types
                            .agent_methods
                            .get(symbol)
                            .ok_or("checkpoint call has no checked agent method selection")?;
                        let expected = agent
                            .select_method(self.checked, selected.agent, selected.body)?
                            .checkpoint_reference()?;
                        if &expected != reference.as_ref() {
                            return Err(
                                "checkpoint agent method differs from its checked call selection"
                                    .into(),
                            );
                        }
                    } else if let etas_hir::HirExpr::Call { callee, .. } = site {
                        let ty = self.expression_type(*callee, bindings)?;
                        self.reference_target_type(target, ty)?;
                    } else {
                        return Err(
                            "checkpoint agent target has no checked callable selection".into()
                        );
                    }
                }
                CallTargetSnapshot::Scoped { .. } => {
                    let etas_hir::HirExpr::Call { callee, .. } = site else {
                        return Err(
                            "checkpoint scoped target has no checked callable expression".into(),
                        );
                    };
                    self.reference_target_type(target, self.expression_type(*callee, bindings)?)?;
                }
                CallTargetSnapshot::Limited { target, .. }
                | CallTargetSnapshot::Specialized { target, .. } => {
                    if target
                        .shared_identity()
                        .is_none_or(|key| seen.insert((0, key as usize)))
                    {
                        pending.push(target);
                    }
                }
                CallTargetSnapshot::Composed(targets) => {
                    if targets
                        .shared_identity()
                        .is_none_or(|key| seen.insert((1, key as usize)))
                    {
                        pending.extend(targets.iter());
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn expression_type(
        &self,
        expr: etas_hir::HirExprId,
        bindings: &crate::control::GenericBindings,
    ) -> Result<TypeId, String> {
        let ty = *self
            .checked
            .types
            .expr_types
            .get(&expr)
            .ok_or("checkpoint call has no checked callee type")?;
        bindings
            .substitution(&self.checked.type_store)
            .substitute(ty)
            .map_err(|error| format!("checkpoint callee type is not materialized: {error}"))
    }

    pub(super) fn validate_local_references(
        &self,
        symbol: SymbolId,
        value: &ValueSnapshot,
        bindings: &crate::control::GenericBindings,
    ) -> Result<(), String> {
        let ty = match self.checked.types.symbol_types.get(&symbol) {
            Some(
                SymbolTypeFact::Local { ty, .. }
                | SymbolTypeFact::Param { ty }
                | SymbolTypeFact::Value { ty }
                | SymbolTypeFact::Field { ty }
                | SymbolTypeFact::TopLevelLet { ty, .. },
            ) => *ty,
            _ => return Err("checkpoint agent local has no checked value type".into()),
        };
        self.reference_value(value, ty, bindings)
    }

    pub(super) fn reference_value(
        &self,
        value: &ValueSnapshot,
        expected: TypeId,
        bindings: &crate::control::GenericBindings,
    ) -> Result<(), String> {
        crate::value::contract::validate(
            self,
            [crate::value::contract::Seed {
                value: crate::value::contract::Value::Snapshot(value),
                ty: expected,
            }],
            bindings,
        )
    }
}

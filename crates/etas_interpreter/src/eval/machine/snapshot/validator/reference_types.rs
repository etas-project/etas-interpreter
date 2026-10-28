use std::collections::HashMap;

use etas_hir::{HirItemId, SymbolId};
use etas_types::{ItemSignature, SymbolTypeFact, Type, TypeId};

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
            if self.pending_direct_intrinsic(call, target, site)? {
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

    pub(crate) fn validate_argument_references(
        &self,
        item: HirItemId,
        values: &[ValueSnapshot],
    ) -> Result<(), String> {
        let signature = match self.checked.types.item_signatures.get(&item) {
            Some(ItemSignature::Flow(signature) | ItemSignature::Tool(signature)) => {
                Some(signature)
            }
            Some(ItemSignature::Agent(agent)) => agent.callable(),
            _ => None,
        }
        .ok_or("checkpoint agent arguments have no checked entry signature")?;
        if signature.params.len() != values.len() {
            return Err("checkpoint agent arguments differ from the checked entry arity".into());
        }
        let bindings = crate::control::GenericBindings::new();
        for (value, ty) in values.iter().zip(&signature.params) {
            self.reference_value(value, *ty, &bindings)?;
        }
        Ok(())
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
        let expected = if bindings.is_empty() {
            expected
        } else {
            bindings
                .substitution(&self.checked.type_store)
                .substitute(expected)
                .map_err(|error| format!("checkpoint slot type is not materialized: {error}"))?
        };
        if !contains_reference(value) && !self.type_requires_reference(expected)? {
            return Ok(());
        }
        self.reference_types(value, expected, bindings)
    }

    pub(super) fn reference_types(
        &self,
        root: &ValueSnapshot,
        expected: TypeId,
        bindings: &crate::control::GenericBindings,
    ) -> Result<(), String> {
        let store = &self.checked.type_store;
        let mut substitution = bindings.substitution(store);
        let mut layouts = crate::plan::code::CheckedValueTypes::default();
        let mut pending = vec![(root, expected)];
        while let Some((value, expected)) = pending.pop() {
            let expected = if bindings.is_empty() {
                expected
            } else {
                substitution.substitute(expected).map_err(|error| {
                    format!("checkpoint reference type is not materialized: {error}")
                })?
            };
            let ty = store
                .get(expected)
                .ok_or("checkpoint reference type is missing")?;
            if let Type::Refined { base, .. } = ty {
                pending.push((value, *base));
                continue;
            }
            if let ValueSnapshot::Callable(target) = value {
                self.reference_target_type(target, expected)?;
                continue;
            }
            if let ValueSnapshot::Nominal { ty: actual, value } = value {
                if *actual != expected {
                    return Err(
                        "checkpoint agent container has a different nominal identity".into(),
                    );
                }
                if matches!(ty, Type::Enum(_))
                    || matches!(ty, Type::Applied { constructor, .. } if matches!(store.get(TypeId(constructor.0)), Some(Type::Enum(_))))
                {
                    pending.push((value, expected));
                } else {
                    let representation = layouts.representation(self.checked, expected).map_err(|error| format!("checkpoint agent container has no checked representation: {error:?}"))?;
                    pending.push((value, representation));
                }
                continue;
            }
            match (ty, value) {
                (Type::Array(inner), ValueSnapshot::Array(values))
                | (Type::List(inner), ValueSnapshot::List(values))
                | (Type::Slice(inner), ValueSnapshot::Slice(values))
                | (Type::Set(inner), ValueSnapshot::Set(values)) => {
                    pending.extend(values.iter().map(|value| (value, *inner)))
                }
                (Type::Option(inner), ValueSnapshot::OptionSome(value)) => {
                    pending.push((value, *inner))
                }
                (Type::Option(_), ValueSnapshot::OptionNone) => {}
                (
                    Type::Trust { wrapper, inner },
                    ValueSnapshot::Trust {
                        wrapper: actual,
                        value,
                    },
                ) if wrapper == actual => pending.push((value, *inner)),
                (Type::Message(inner), ValueSnapshot::Message(message)) => {
                    pending.push((&message.payload, *inner))
                }
                (Type::Tuple(types), ValueSnapshot::Tuple(values))
                    if types.len() == values.len() =>
                {
                    pending.extend(values.iter().zip(types).map(|(value, ty)| (value, *ty)))
                }
                (Type::Record(record), ValueSnapshot::Record(fields)) => {
                    if record.fields.len() != fields.len() {
                        return Err("checkpoint record differs from its checked field count".into());
                    }
                    let types: HashMap<_, _> = record
                        .fields
                        .iter()
                        .map(|field| (field.name.as_str(), field.ty))
                        .collect();
                    let mut names = std::collections::HashSet::new();
                    for (name, value) in fields.iter() {
                        if !names.insert(name) {
                            return Err("checkpoint agent record has duplicate fields".into());
                        }
                        match types.get(name.as_str()) {
                            Some(ty) => pending.push((value, *ty)),
                            None => {
                                return Err(
                                    "checkpoint agent record field has no checked type".into()
                                );
                            }
                        }
                    }
                }
                (Type::Map { key, value: ty }, ValueSnapshot::Map(entries)) => {
                    for (a, b) in entries.iter() {
                        pending.push((a, *key));
                        pending.push((b, *ty));
                    }
                }
                (Type::Result { ok, err }, ValueSnapshot::Variant { name, fields })
                    if fields.len() == 1 && matches!(name.as_str(), "Ok" | "Err") =>
                {
                    pending.push((&fields[0], if name == "Ok" { *ok } else { *err }));
                }
                (Type::Enum(_) | Type::Applied { .. }, ValueSnapshot::Variant { name, fields }) => {
                    let variant =
                        layouts
                            .variant(self.checked, expected, name)
                            .map_err(|error| {
                                format!("checkpoint agent variant has no checked layout: {error:?}")
                            })?;
                    if variant
                        .names
                        .as_ref()
                        .is_some_and(|names| names.len() != variant.fields.len())
                    {
                        return Err(
                            "checkpoint agent variant has an inconsistent checked named layout"
                                .into(),
                        );
                    }
                    if fields.len() != variant.fields.len() {
                        return Err("checkpoint agent variant has invalid payload arity".into());
                    }
                    // Both source payload spellings lower to declaration-order
                    // fields in the checked enum ABI.
                    pending.extend(
                        fields
                            .iter()
                            .zip(&variant.fields)
                            .map(|(value, ty)| (value, *ty)),
                    );
                }
                _ if contains_reference(value) || self.type_requires_reference(expected)? => {
                    return Err(
                        "checkpoint agent value has no checked container type projection".into(),
                    );
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn contains_reference(value: &ValueSnapshot) -> bool {
    value
        .walk()
        .any(|value| matches!(value, ValueSnapshot::Callable(_)))
}

use super::*;
use etas_hir::HirExpr;
use etas_types::Type;
use std::collections::{HashMap, HashSet};

impl SnapshotValidator<'_> {
    pub(super) fn construction_continuation(
        &self,
        continuation: &ContinuationSnapshot,
        context: &str,
    ) -> Result<(), String> {
        match continuation {
            ContinuationSnapshot::AggregateElement {
                expr,
                next_index,
                values,
                frame,
            } => {
                let elems = match self.checked.hir.exprs.get(*expr) {
                    Some(
                        HirExpr::Tuple { elems, .. }
                        | HirExpr::Array { elems, .. }
                        | HirExpr::List { elems, .. }
                        | HirExpr::Set { elems, .. },
                    ) => elems,
                    _ => return Err(format!("{context}: missing checked aggregate construction")),
                };
                self.index_at_most(*next_index, elems.len(), context)?;
                if next_index.checked_sub(1) != Some(values.len()) {
                    return Err(format!(
                        "{context}: inconsistent evaluated aggregate elements"
                    ));
                }
                let bindings = frame.generic_bindings();
                for (value, expr) in values.iter().zip(elems) {
                    self.typed_expression_value(value, *expr, &bindings)?;
                }
                self.frame(frame, context)
            }
            ContinuationSnapshot::RecordField {
                expr,
                nominal_type,
                variant_symbol,
                next_index,
                values,
                frame,
            } => {
                let fields =
                    self.record_construction(*expr, *nominal_type, *variant_symbol, context)?;
                if let Some(ty) = nominal_type {
                    self.type_id(*ty, &format!("{context} nominal record type"))?;
                }
                if let Some(symbol) = variant_symbol {
                    self.named_variant_fields(*symbol, *nominal_type, fields, context)?;
                }
                self.fields(fields, context)?;
                self.index_at_most(*next_index, fields.len(), context)?;
                let pending = next_index.checked_sub(1).ok_or_else(|| {
                    format!("{context}: record continuation has no pending field")
                })?;
                if !matches!(fields.get(pending), Some(HirFieldInit::Named { .. }))
                    || values.len() != pending
                    || values.iter().zip(fields).any(|((actual, _), field)| {
                        let (HirFieldInit::Named { name, .. }
                        | HirFieldInit::Shorthand { name, .. }) = field;
                        actual != name
                    })
                {
                    return Err(format!(
                        "{context}: record continuation has inconsistent evaluated fields"
                    ));
                }
                let types = self.construction_fields(*expr, *variant_symbol)?;
                let bindings = frame.generic_bindings();
                for (name, value) in values {
                    let ty = *types
                        .get(name)
                        .ok_or("checkpoint record field has no checked type")?;
                    self.snapshot_value_in(value, call_target::ReferenceScope::CheckedSlot)?;
                    self.reference_value(value, ty, &bindings)?;
                }
                self.frame(frame, context)
            }
            ContinuationSnapshot::MapKey {
                expr,
                index,
                values,
                frame,
            }
            | ContinuationSnapshot::MapValue {
                expr,
                index,
                values,
                frame,
                ..
            } => {
                let Some(HirExpr::Map { entries, .. }) = self.checked.hir.exprs.get(*expr) else {
                    return Err(format!("{context}: missing checked map construction"));
                };
                self.index_below(*index, entries.len(), context)?;
                if values.len() != *index {
                    return Err(format!("{context}: inconsistent evaluated map entries"));
                }
                let bindings = frame.generic_bindings();
                for ((key, value), entry) in values.iter().zip(entries) {
                    self.typed_expression_value(key, entry.key, &bindings)?;
                    self.typed_expression_value(value, entry.value, &bindings)?;
                }
                if let ContinuationSnapshot::MapValue { key, .. } = continuation {
                    self.typed_expression_value(key, entries[*index].key, &bindings)?;
                }
                self.frame(frame, context)
            }
            _ => Err(format!("{context}: expected a construction continuation")),
        }
    }

    fn construction_fields(
        &self,
        expr: HirExprId,
        variant: Option<SymbolId>,
    ) -> Result<HashMap<String, TypeId>, String> {
        let mut ty = *self
            .checked
            .types
            .expr_types
            .get(&expr)
            .ok_or("checkpoint record construction has no checked type")?;
        let mut layouts = crate::plan::code::CheckedValueTypes::default();
        if let Some(variant) = variant {
            let symbol = self
                .checked
                .symbols
                .get(variant)
                .ok_or("checkpoint enum constructor has no checked symbol")?;
            let variant = layouts
                .variant(self.checked, ty, &symbol.name)
                .map_err(|error| {
                    format!("checkpoint named enum has no checked layout: {error:?}")
                })?;
            let names = variant
                .names
                .as_ref()
                .filter(|names| names.len() == variant.fields.len())
                .ok_or("checkpoint enum constructor has no checked named field layout")?;
            return Ok(names
                .iter()
                .cloned()
                .zip(variant.fields.iter().copied())
                .collect());
        }
        // Nominal field projection is allowed here; it does not make the
        // nominal value assignable to its representation.
        let mut seen = HashSet::new();
        while seen.insert(ty) {
            match self.checked.type_store.get(ty) {
                Some(Type::Record(record)) => {
                    return Ok(record
                        .fields
                        .iter()
                        .map(|field| (field.name.clone(), field.ty))
                        .collect());
                }
                Some(Type::Refined { base, .. }) => ty = *base,
                Some(Type::Nominal(_) | Type::Applied { .. }) => {
                    ty = layouts.representation(self.checked, ty).map_err(|error| {
                        format!("checkpoint record has no checked representation: {error:?}")
                    })?;
                }
                _ => {
                    return Err(
                        "checkpoint construction does not have a checked record layout".into(),
                    );
                }
            }
        }
        Err("checkpoint record representation is cyclic".into())
    }
}

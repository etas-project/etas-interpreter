use std::collections::HashMap;

use etas_frontend::CheckedProject;
use etas_hir::{
    HirExpr, HirExprId, HirFieldInit, HirItem, HirPat, HirPatId, HirRecordPatField, ResolveResult,
    SymbolDef, SymbolId,
};
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use super::context::{PlanContext, plan_pass};
use crate::value::InterpValue;

#[derive(Clone, Debug)]
pub(crate) struct NamedVariantLayout {
    symbol: SymbolId,
    source_names: Vec<String>,
    swaps: Vec<(usize, usize)>,
}

impl NamedVariantLayout {
    fn build(symbol: SymbolId, declared: &[String], source: Vec<String>) -> Result<Self, String> {
        if declared.len() != source.len() {
            return Err("named variant field count differs from checked declaration".into());
        }
        let mut slots: HashMap<_, _> = declared
            .iter()
            .enumerate()
            .map(|(index, name)| (name.as_str(), index))
            .collect();
        if slots.len() != declared.len() {
            return Err("named variant declaration has duplicate fields".into());
        }
        let mut destinations = source
            .iter()
            .map(|name| {
                slots
                    .remove(name.as_str())
                    .ok_or_else(|| format!("unknown or duplicate named variant field `{name}`"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut swaps = Vec::new();
        for index in 0..destinations.len() {
            while destinations[index] != index {
                let target = destinations[index];
                swaps.push((index, target));
                destinations.swap(index, target);
            }
        }
        Ok(Self {
            symbol,
            source_names: source,
            swaps,
        })
    }

    pub(crate) fn reorder(
        &self,
        symbol: SymbolId,
        values: Vec<(String, InterpValue)>,
    ) -> Result<Vec<InterpValue>, String> {
        if symbol != self.symbol || values.len() != self.source_names.len() {
            return Err("named variant does not match checked constructor layout".into());
        }
        let mut fields = values
            .into_iter()
            .zip(&self.source_names)
            .map(|((name, value), expected)| {
                if &name != expected {
                    return Err(
                        "named variant values do not follow checked source field order".to_owned(),
                    );
                }
                Ok(value)
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (left, right) in &self.swaps {
            fields.swap(*left, *right);
        }
        Ok(fields)
    }
}

#[derive(Clone, Debug, Default)]
pub struct NamedVariantLayoutTable {
    layouts: HashMap<HirExprId, NamedVariantLayout>,
    patterns: HashMap<HirPatId, NamedVariantPatternLayout>,
}

#[derive(Clone, Debug)]
pub(crate) struct NamedVariantPatternLayout {
    pub symbol: SymbolId,
    pub name: String,
    pub arity: usize,
    pub fields: Vec<(usize, HirPatId, etas_core::Span)>,
}

impl NamedVariantPatternLayout {
    fn build(
        symbol: SymbolId,
        name: &str,
        declared: &[String],
        fields: &[HirRecordPatField],
    ) -> Result<Self, String> {
        let mut slots: HashMap<_, _> = declared
            .iter()
            .enumerate()
            .map(|(slot, name)| (name.as_str(), slot))
            .collect();
        if slots.len() != declared.len() {
            return Err("named variant declaration has duplicate fields".into());
        }
        let mut bindings = Vec::with_capacity(fields.len());
        for field in fields {
            let slot = slots.remove(field.name.as_str()).ok_or_else(|| {
                format!(
                    "unknown or duplicate named variant pattern field `{}`",
                    field.name
                )
            })?;
            if let Some(pat) = field.pat {
                bindings.push((slot, pat, field.span));
            }
        }
        Ok(Self {
            symbol,
            name: name.to_owned(),
            arity: declared.len(),
            fields: bindings,
        })
    }
}

impl NamedVariantLayoutTable {
    pub(crate) fn get(&self, expr: HirExprId) -> Option<&NamedVariantLayout> {
        self.layouts.get(&expr)
    }

    pub(crate) fn pattern(&self, pat: HirPatId) -> Option<&NamedVariantPatternLayout> {
        self.patterns.get(&pat)
    }

    fn build(project: &CheckedProject) -> Result<Self, String> {
        let mut layouts = HashMap::new();
        for (expr, data) in project.hir.exprs.iter() {
            let HirExpr::Record(record) = data else {
                continue;
            };
            let Some(path) = &record.path else {
                continue;
            };
            let ResolveResult::Resolved(symbol) = path.resolution else {
                continue;
            };
            let symbol_data = project
                .symbols
                .get(symbol)
                .ok_or("record constructor has an invalid symbol")?;
            let SymbolDef::EnumVariant {
                enum_item,
                variant_index,
            } = symbol_data.def
            else {
                continue;
            };
            let Some(HirItem::Enum(declaration)) = project.hir.items.get(enum_item) else {
                return Err("variant constructor has no enum declaration".into());
            };
            let names = declaration
                .variants
                .get(variant_index as usize)
                .and_then(|variant| variant.field_names.as_ref())
                .ok_or("named variant has no checked field declaration")?;
            let source = record
                .fields
                .iter()
                .map(|field| match field {
                    HirFieldInit::Named { name, .. } | HirFieldInit::Shorthand { name, .. } => {
                        name.clone()
                    }
                })
                .collect();
            layouts.insert(expr, NamedVariantLayout::build(symbol, names, source)?);
        }
        let mut patterns = HashMap::new();
        for (pat, data) in project.hir.pats.iter() {
            let HirPat::Record {
                path: Some(path),
                fields,
                ..
            } = data
            else {
                continue;
            };
            let ResolveResult::Resolved(symbol) = path.resolution else {
                continue;
            };
            let symbol_data = project
                .symbols
                .get(symbol)
                .ok_or("record pattern has an invalid symbol")?;
            let SymbolDef::EnumVariant {
                enum_item,
                variant_index,
            } = symbol_data.def
            else {
                continue;
            };
            let Some(HirItem::Enum(declaration)) = project.hir.items.get(enum_item) else {
                return Err("variant pattern has no enum declaration".into());
            };
            let names = declaration
                .variants
                .get(variant_index as usize)
                .and_then(|variant| variant.field_names.as_ref())
                .ok_or("named variant pattern has no checked field declaration")?;
            patterns.insert(
                pat,
                NamedVariantPatternLayout::build(symbol, &symbol_data.name, names, fields)?,
            );
        }
        Ok(Self { layouts, patterns })
    }
}

pub(super) struct BuildNamedVariantLayoutsPass;

impl Pass<PlanContext<'_>> for BuildNamedVariantLayoutsPass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.build_named_variant_layouts")
    }
    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _: &PassContext<PlanContext<'_>>,
        _: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        match NamedVariantLayoutTable::build(context.project) {
            Ok(layouts) => context.named_variants = Some(layouts),
            Err(error) => {
                context.push_missing_fact(crate::diagnostics::primary_span(context.project), &error)
            }
        }
        PassResult::unchanged()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::allocation::measure;

    #[test]
    fn named_pattern_slots_follow_declared_order_without_runtime_name_search() {
        let span = etas_core::Span::new(
            etas_core::SourceId(0),
            etas_core::TextRange::new(etas_core::TextSize(0), etas_core::TextSize(1)),
        );
        for count in [1000, 2000, 4000] {
            let declared = (0..count).map(|i| format!("field{i}")).collect::<Vec<_>>();
            let fields = declared
                .iter()
                .enumerate()
                .rev()
                .map(|(i, name)| HirRecordPatField {
                    name: name.clone(),
                    pat: Some(HirPatId(i as u32)),
                    span,
                })
                .collect::<Vec<_>>();
            let layout =
                NamedVariantPatternLayout::build(SymbolId(1), "Fields", &declared, &fields)
                    .unwrap();
            let payload = (0..count)
                .map(|i| InterpValue::i32(i as i32))
                .collect::<Vec<_>>();
            let (sum, allocations) = measure(|| {
                layout
                    .fields
                    .iter()
                    .map(|(slot, pat, _)| {
                        assert_eq!(*slot, pat.0 as usize);
                        let InterpValue::Number(crate::value::NumericValue::I32(value)) =
                            &payload[*slot]
                        else {
                            panic!("wrong payload")
                        };
                        *value as usize
                    })
                    .sum::<usize>()
            });
            assert_eq!(sum, count * (count - 1) / 2);
            assert_eq!(allocations.count, 0);
            let mut duplicate = fields.clone();
            duplicate[0].name = duplicate[1].name.clone();
            assert!(
                NamedVariantPatternLayout::build(SymbolId(1), "Fields", &declared, &duplicate)
                    .is_err()
            );
        }
    }

    #[test]
    fn named_variant_permutation_moves_payloads_without_copying() {
        for count in [1000, 2000, 4000] {
            let declared: Vec<_> = (0..count).map(|index| format!("field{index}")).collect();
            let source: Vec<_> = declared.iter().rev().cloned().collect();
            let layout = NamedVariantLayout::build(SymbolId(7), &declared, source.clone()).unwrap();
            let values: Vec<_> = source
                .into_iter()
                .map(|name| (name, InterpValue::String("x".repeat(128).into())))
                .collect();
            let pointers: Vec<_> = values
                .iter()
                .rev()
                .map(|(_, value)| match value {
                    InterpValue::String(s) => s.as_ptr(),
                    _ => unreachable!(),
                })
                .collect();
            let (result, allocations) = measure(|| layout.reorder(SymbolId(7), values).unwrap());
            assert!(
                allocations.count <= 1,
                "only the result vector may allocate: {allocations:?}"
            );
            for (value, pointer) in result.iter().zip(pointers) {
                let InterpValue::String(value) = value else {
                    panic!("payload type changed")
                };
                assert_eq!(value.as_ptr(), pointer);
            }
            assert!(layout.swaps.len() < count);
        }
    }

    #[test]
    fn named_variant_layout_rejects_mismatched_fields_and_constructor_identity() {
        let names = vec!["a".into(), "b".into()];
        assert!(
            NamedVariantLayout::build(SymbolId(7), &names, vec!["a".into(), "a".into()]).is_err()
        );
        assert!(
            NamedVariantLayout::build(SymbolId(7), &names, vec!["a".into(), "c".into()]).is_err()
        );
        assert!(NamedVariantLayout::build(SymbolId(7), &names, vec!["a".into()]).is_err());
        let layout = NamedVariantLayout::build(SymbolId(7), &names, names.clone()).unwrap();
        let values = || {
            vec![
                ("a".into(), InterpValue::Unit),
                ("b".into(), InterpValue::Unit),
            ]
        };
        assert!(layout.reorder(SymbolId(8), values()).is_err());
        let mut reordered = values();
        reordered.swap(0, 1);
        assert!(layout.reorder(SymbolId(7), reordered).is_err());
    }
}

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use etas_core::Span;
use etas_frontend::CheckedProject;
use etas_hir::{
    HirExpr, HirExprId, HirFieldInit, HirPat, HirPatId, PartialResolutionReason, ResolveResult,
    SymbolId,
};
use etas_types::{SymbolTypeFact, Type, TypeId, TypeStore};
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use super::context::{PlanContext, plan_pass};
use crate::value::{InterpValue, RecordValue, record::RecordLayout};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CheckedRecordField {
    names: Arc<[String]>,
    slot: usize,
}

impl CheckedRecordField {
    pub(crate) fn borrow<'a>(
        &self,
        value: &'a RecordValue,
    ) -> Result<std::cell::Ref<'a, InterpValue>, &'static str> {
        value.borrow_checked_field(self.slot, &self.names[self.slot], self.names.len())
    }

    pub(crate) fn name(&self) -> &str {
        &self.names[self.slot]
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RecordPatternField {
    pub field: CheckedRecordField,
    pub pat: Option<HirPatId>,
    pub span: Span,
}

#[derive(Clone, Debug)]
struct RecordConstruction {
    names: Vec<String>,
    layout: Arc<RecordLayout>,
}

#[derive(Clone, Copy)]
pub(crate) enum FieldAccessSite<'a> {
    Expr(HirExprId),
    Path {
        prefix: SymbolId,
        remaining: &'a [String],
        index: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PathField {
    field: Option<CheckedRecordField>,
    output: TypeId,
}

type PathFields = HashMap<SymbolId, HashMap<Vec<String>, Vec<PathField>>>;

#[derive(Clone, Debug, Default)]
pub struct RecordLayoutTable {
    constructions: HashMap<HirExprId, RecordConstruction>,
    fields: HashMap<HirExprId, CheckedRecordField>,
    patterns: HashMap<HirPatId, Vec<RecordPatternField>>,
    paths: PathFields,
}

impl RecordLayoutTable {
    pub(crate) fn field(&self, site: FieldAccessSite<'_>) -> Option<&CheckedRecordField> {
        match site {
            FieldAccessSite::Expr(expr) => self.fields.get(&expr),
            FieldAccessSite::Path {
                prefix,
                remaining,
                index,
            } => self
                .paths
                .get(&prefix)?
                .get(remaining)?
                .get(index)?
                .field
                .as_ref(),
        }
    }

    pub(crate) fn field_type(
        &self,
        project: &CheckedProject,
        site: FieldAccessSite<'_>,
    ) -> Option<TypeId> {
        match site {
            FieldAccessSite::Expr(expr) => project.types.expr_types.get(&expr).copied(),
            FieldAccessSite::Path {
                prefix,
                remaining,
                index,
            } => Some(self.paths.get(&prefix)?.get(remaining)?.get(index)?.output),
        }
    }

    pub(crate) fn pattern(&self, pat: HirPatId) -> Option<&[RecordPatternField]> {
        self.patterns.get(&pat).map(Vec::as_slice)
    }

    pub(crate) fn construct(
        &self,
        expr: HirExprId,
        fields: Vec<(String, InterpValue)>,
    ) -> Result<InterpValue, String> {
        let layout = self
            .constructions
            .get(&expr)
            .ok_or("record constructor has no checked layout")?;
        if fields.len() != layout.names.len()
            || fields
                .iter()
                .zip(&layout.names)
                .any(|((name, _), expected)| name != expected)
        {
            return Err("record values do not follow checked source field order".into());
        }
        Ok(InterpValue::Record(RecordValue::with_layout(
            fields,
            layout.layout.clone(),
        )))
    }

    fn build(project: &CheckedProject) -> Result<Self, String> {
        let mut table = Self::default();
        let mut shapes = RecordShapes::new(&project.type_store);
        for (expr, data) in project.hir.exprs.iter() {
            match data {
                HirExpr::Record(record) => {
                    let Some(ty) = project.types.expr_types.get(&expr) else {
                        continue;
                    };
                    let Some(names) = shapes.get(*ty)? else {
                        continue;
                    };
                    let source: Vec<_> = record
                        .fields
                        .iter()
                        .map(|field| match field {
                            HirFieldInit::Named { name, .. }
                            | HirFieldInit::Shorthand { name, .. } => name.clone(),
                        })
                        .collect();
                    let mut sorted = source.clone();
                    sorted.sort();
                    if sorted.as_slice() != names.as_ref() {
                        return Err("record constructor fields differ from checked type".into());
                    }
                    table.constructions.insert(
                        expr,
                        RecordConstruction {
                            layout: Arc::new(RecordLayout::from_names(&source)),
                            names: source,
                        },
                    );
                }
                HirExpr::Field { base, field, .. } => {
                    let Some(ty) = project.types.expr_types.get(base) else {
                        continue;
                    };
                    let Some(names) = shapes.get(*ty)? else {
                        continue;
                    };
                    let projections = project
                        .types
                        .field_projections
                        .get(&expr)
                        .ok_or("record field has no checked projection fact")?;
                    if projections.len() != 1
                        || projections[0].field != *field
                        || projections[0].receiver != *ty
                    {
                        return Err(format!(
                            "record field `{field}` differs from checked projection fact"
                        ));
                    }
                    table.fields.insert(expr, checked_field(names, field)?);
                }
                HirExpr::Path(path) => {
                    let ResolveResult::PartiallyResolved(partial) = &path.resolution else {
                        continue;
                    };
                    if partial.reason != PartialResolutionReason::MemberRequiresTypeChecking {
                        continue;
                    }
                    let Some(prefix) = partial.resolved_prefix else {
                        continue;
                    };
                    let Some(
                        SymbolTypeFact::Param { ty }
                        | SymbolTypeFact::Local { ty, .. }
                        | SymbolTypeFact::Value { ty }
                        | SymbolTypeFact::Field { ty }
                        | SymbolTypeFact::TopLevelLet { ty, .. },
                    ) = project.types.symbol_types.get(&prefix)
                    else {
                        continue;
                    };
                    if !project.types.expr_types.contains_key(&expr) {
                        continue;
                    }
                    let projections = project
                        .types
                        .field_projections
                        .get(&expr)
                        .ok_or("value path has no checked field projection facts")?;
                    if projections.len() != partial.remaining.len() {
                        return Err(
                            "value path length differs from checked projection facts".into()
                        );
                    }
                    let mut current = *ty;
                    let mut fields = Vec::with_capacity(partial.remaining.len());
                    for (name, projection) in partial.remaining.iter().zip(projections) {
                        if projection.receiver != current || projection.field != *name {
                            return Err(format!(
                                "value path field `{name}` differs from checked projection facts"
                            ));
                        }
                        let field = shapes
                            .get(projection.receiver)?
                            .map(|names| checked_field(names, name))
                            .transpose()?;
                        current = projection.output;
                        if project.type_store.get(current).is_none() {
                            return Err("checked field projection output type is missing".into());
                        }
                        fields.push(PathField {
                            field,
                            output: current,
                        });
                    }
                    let paths = table.paths.entry(prefix).or_default();
                    if paths
                        .get(&partial.remaining)
                        .is_some_and(|old| old != &fields)
                    {
                        return Err(
                            "value path has conflicting checked field projection facts".into()
                        );
                    }
                    paths.insert(partial.remaining.clone(), fields);
                }
                _ => {}
            }
        }
        for (pat, data) in project.hir.pats.iter() {
            let HirPat::Record { fields, .. } = data else {
                continue;
            };
            let Some(ty) = project.types.pattern_types.get(&pat) else {
                continue;
            };
            let Some(names) = shapes.get(*ty)? else {
                continue;
            };
            let mut seen = HashSet::new();
            let mut layout = Vec::with_capacity(fields.len());
            for field in fields {
                if !seen.insert(&field.name) {
                    return Err(format!(
                        "duplicate checked record pattern field `{}`",
                        field.name
                    ));
                }
                layout.push(RecordPatternField {
                    field: checked_field(names.clone(), &field.name)?,
                    pat: field.pat,
                    span: field.span,
                });
            }
            table.patterns.insert(pat, layout);
        }
        Ok(table)
    }
}

fn checked_field(names: Arc<[String]>, name: &str) -> Result<CheckedRecordField, String> {
    let slot = names
        .binary_search_by(|candidate| candidate.as_str().cmp(name))
        .map_err(|_| format!("record field `{name}` does not exist in checked type"))?;
    Ok(CheckedRecordField { names, slot })
}

struct RecordShapes<'a> {
    store: &'a TypeStore,
    cache: HashMap<TypeId, Option<Arc<[String]>>>,
}

impl<'a> RecordShapes<'a> {
    fn new(store: &'a TypeStore) -> Self {
        Self {
            store,
            cache: HashMap::new(),
        }
    }

    fn get(&mut self, ty: TypeId) -> Result<Option<Arc<[String]>>, String> {
        if let Some(shape) = self.cache.get(&ty) {
            return Ok(shape.clone());
        }
        let mut current = ty;
        let mut visited = HashSet::new();
        let result = loop {
            if !visited.insert(current) {
                return Err("cyclic nominal representation in checked record layout".into());
            }
            match self
                .store
                .get(current)
                .ok_or("record layout references a missing checked type")?
            {
                Type::Record(record) => {
                    let mut names: Vec<_> = record
                        .fields
                        .iter()
                        .map(|field| field.name.clone())
                        .collect();
                    names.sort();
                    if names.windows(2).any(|pair| pair[0] == pair[1]) {
                        return Err("checked record type has duplicate field names".into());
                    }
                    break Some(Arc::<[String]>::from(names));
                }
                Type::Nominal(nominal) => match nominal.representation {
                    Some(representation) => current = representation,
                    None => break None,
                },
                // Generic arguments change field types, never the declared field names.
                Type::Applied { constructor, .. } => current = TypeId(constructor.0),
                Type::Refined { base, .. } => current = *base,
                _ => break None,
            }
        };
        self.cache.insert(ty, result.clone());
        Ok(result)
    }
}

pub(super) struct BuildRecordLayoutsPass;

impl Pass<PlanContext<'_>> for BuildRecordLayoutsPass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.build_record_layouts")
    }
    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _: &PassContext<PlanContext<'_>>,
        _: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        match RecordLayoutTable::build(context.project) {
            Ok(layouts) => context.records = Some(layouts),
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
    fn record_construction_and_checked_queries_do_not_clone_unselected_fields() {
        for count in [1000, 2000, 4000] {
            let names: Arc<[String]> = (0..count).map(|i| format!("field_{i:04}")).collect();
            let source: Vec<_> = names.iter().rev().cloned().collect();
            let mut table = RecordLayoutTable::default();
            let expr = HirExprId(0);
            table.constructions.insert(
                expr,
                RecordConstruction {
                    layout: Arc::new(RecordLayout::from_names(&source)),
                    names: source.clone(),
                },
            );
            let fields: Vec<_> = source
                .into_iter()
                .map(|name| (name, InterpValue::String("x".repeat(128).into())))
                .collect();
            let pointers: Vec<_> = fields
                .iter()
                .map(|(_, value)| match value {
                    InterpValue::String(value) => value.as_ptr(),
                    _ => unreachable!(),
                })
                .rev()
                .collect();
            let (value, allocations) = measure(|| table.construct(expr, fields).unwrap());
            assert_eq!(
                allocations.count, 1,
                "only the record backing, no new field layout"
            );
            let InterpValue::Record(value) = value else {
                panic!("record");
            };
            let fields: Vec<_> = names
                .iter()
                .map(|name| checked_field(names.clone(), name).unwrap())
                .collect();
            let (_, allocations) = measure(|| {
                for (field, pointer) in fields.iter().zip(pointers) {
                    let value = field.borrow(&value).unwrap();
                    let InterpValue::String(value) = &*value else {
                        panic!("string");
                    };
                    assert_eq!(value.as_ptr(), pointer);
                }
            });
            assert_eq!(allocations.count, 0);
            assert_eq!(value.borrow()[0].0, format!("field_{:04}", count - 1));
            let (_, allocations) = measure(|| value.get(&names[count - 1]).unwrap());
            assert_eq!(allocations.count, 0, "selected text shares its backing");
        }
    }

    #[test]
    fn record_slots_rebuild_after_mutation_and_do_not_define_type_identity() {
        let names: Arc<[String]> = vec!["a".into(), "b".into()].into();
        let a = checked_field(names.clone(), "a").unwrap();
        let mut value = RecordValue::new(vec![
            ("b".into(), InterpValue::i32(2)),
            ("a".into(), InterpValue::i32(1)),
        ]);
        assert_eq!(*a.borrow(&value).unwrap(), InterpValue::i32(1));
        let alias = value.clone();
        value.make_unique();
        let (_, allocations) = crate::testing::allocation::measure(|| {
            *value.field_mut("a").unwrap() = InterpValue::i32(3);
            assert_eq!(*a.borrow(&value).unwrap(), InterpValue::i32(3));
        });
        assert_eq!(
            allocations.count, 0,
            "value-only writes retain the shared slot layout"
        );
        value.borrow_mut().swap(0, 1);
        value.borrow_mut()[0].1 = InterpValue::i32(3);
        assert_eq!(*a.borrow(&value).unwrap(), InterpValue::i32(3));
        assert_eq!(*a.borrow(&alias).unwrap(), InterpValue::i32(1));
        value.borrow_mut()[0].0 = "unknown".into();
        assert!(a.borrow(&value).is_err());
        assert!(value.get("a").is_none());
        assert_eq!(value.get("unknown"), Some(InterpValue::i32(3)));
        assert!(checked_field(names, "missing").is_err());
        assert!(a.borrow(&RecordValue::new(vec![])).is_err());
    }
}

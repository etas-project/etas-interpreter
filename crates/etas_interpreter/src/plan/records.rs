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
    pub(crate) fn borrow<'a>(&self, value: &'a RecordValue) -> Result<&'a InterpValue, String> {
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
    range: bool,
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
    native: Option<etas_std::RuntimeField>,
    value: Option<etas_types::value_representation::ValueProjection>,
    output: TypeId,
}

type PathFields = HashMap<SymbolId, HashMap<Vec<String>, Vec<PathField>>>;

fn source_memory_field(project: &CheckedProject, field: &etas_types::FieldProjectionFact) -> bool {
    use etas_types::value_representation::{memory_region_schema, source_memory_store_parameters};
    memory_region_schema(&project.type_store, field.receiver).is_some()
        && source_memory_store_parameters(&project.type_store, field.output).is_some()
}

#[derive(Clone, Debug, Default)]
pub struct RecordLayoutTable {
    source_iterators: HashMap<HirExprId, super::iteration::SourceIteratorProtocol>,
    constructions: HashMap<HirExprId, RecordConstruction>,
    primitives: HashMap<HirExprId, TypeId>,
    fields: HashMap<HirExprId, CheckedRecordField>,
    native_fields: HashMap<HirExprId, etas_std::RuntimeField>,
    value_fields: HashMap<HirExprId, etas_types::value_representation::ValueProjection>,
    patterns: HashMap<HirPatId, Vec<RecordPatternField>>,
    paths: PathFields,
}

impl RecordLayoutTable {
    pub(crate) fn range_constructor(&self, expr: HirExprId) -> bool {
        self.constructions
            .get(&expr)
            .is_some_and(|construction| construction.range)
    }

    pub(crate) fn source_iterator(
        &self,
        expr: HirExprId,
    ) -> Option<&super::iteration::SourceIteratorProtocol> {
        self.source_iterators.get(&expr)
    }
    pub(crate) fn primitive_constructor(&self, expr: HirExprId) -> Option<TypeId> {
        self.primitives.get(&expr).copied()
    }
    pub(crate) fn value_projection(
        &self,
        site: FieldAccessSite<'_>,
    ) -> Option<etas_types::value_representation::ValueProjection> {
        match site {
            FieldAccessSite::Expr(expr) => self.value_fields.get(&expr).copied(),
            FieldAccessSite::Path {
                prefix,
                remaining,
                index,
            } => self.paths.get(&prefix)?.get(remaining)?.get(index)?.value,
        }
    }

    pub(crate) fn native_field(&self, site: FieldAccessSite<'_>) -> Option<etas_std::RuntimeField> {
        match site {
            FieldAccessSite::Expr(expr) => self.native_fields.get(&expr).copied(),
            FieldAccessSite::Path {
                prefix,
                remaining,
                index,
            } => self.paths.get(&prefix)?.get(remaining)?.get(index)?.native,
        }
    }

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
            FieldAccessSite::Expr(expr) => project.types.expression_producer_type(expr),
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
        if layout.range {
            let mut fields = fields
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>();
            let start = Box::new(fields.remove("start").unwrap());
            let end = Box::new(fields.remove("end").unwrap());
            let (Some(InterpValue::Bool(include_start)), Some(InterpValue::Bool(include_end))) =
                (fields.remove("include_start"), fields.remove("include_end"))
            else {
                return Err("range boundary fields differ from checked bool inputs".into());
            };
            use crate::value::RangeBounds;
            let bounds = match (include_start, include_end) {
                (true, true) => RangeBounds::ClosedClosed,
                (true, false) => RangeBounds::ClosedOpen,
                (false, true) => RangeBounds::OpenClosed,
                (false, false) => RangeBounds::OpenOpen,
            };
            return Ok(InterpValue::Range(crate::value::RangeValue {
                start,
                end,
                bounds,
            }));
        }
        Ok(InterpValue::Record(RecordValue::with_layout(
            fields,
            layout.layout.clone(),
        )))
    }

    fn build(project: &CheckedProject) -> Result<Self, String> {
        let mut table = Self::default();
        let spec_facts = etas_types::SpecFacts::from(project.types.as_ref());
        let mut shapes = RecordShapes::new(&project.type_store);
        let mut iterator_types = None;
        // An entry-scoped check keeps HIR for other flows, but only publishes
        // body facts for the selected execution closure.
        for stmt in project.types.stmt_types.keys() {
            if let etas_hir::HirStmt::For { iter, .. } = &project.hir.stmts[*stmt] {
                let lookup = iterator_types.get_or_insert_with(|| {
                    etas_types::ty::TypeLookupIndex::new(&project.type_store)
                });
                match super::iteration::SourceIteratorProtocol::checked(
                    project,
                    *iter,
                    Some(lookup),
                )? {
                    Some(protocol) => {
                        table.source_iterators.insert(*iter, protocol);
                    }
                    None if project.hir.source_standard_types => {
                        return Err("source for-loop is missing its checked cursor contract".into());
                    }
                    None => {}
                }
            }
        }
        for (expr, data) in project.hir.exprs.iter() {
            match data {
                HirExpr::Record(record) => {
                    let Some(ty) = project.types.expression_producer_type(expr) else {
                        continue;
                    };
                    if let Some(input) = etas_types::value_representation::checked_primitive_input(
                        &project.type_store,
                        &spec_facts,
                        ty,
                    ) {
                        if record.path.is_none() {
                            return Err(
                                "primitive construction requires an explicit checked type path"
                                    .into(),
                            );
                        }
                        let [field] = record.fields.as_slice() else {
                            return Err("primitive construction requires one field".into());
                        };
                        let (name, value) = match field {
                            HirFieldInit::Named { name, value, .. } => {
                                (name, project.types.expression_producer_type(*value))
                            }
                            HirFieldInit::Shorthand {
                                name,
                                resolution: ResolveResult::Resolved(symbol),
                                ..
                            } => (
                                name,
                                project.types.symbol_types.get(symbol).and_then(
                                    |fact| match fact {
                                        SymbolTypeFact::Param { ty }
                                        | SymbolTypeFact::Local { ty, .. }
                                        | SymbolTypeFact::Value { ty }
                                        | SymbolTypeFact::Field { ty }
                                        | SymbolTypeFact::TopLevelLet { ty, .. } => Some(*ty),
                                        _ => None,
                                    },
                                ),
                            ),
                            _ => return Err("primitive constructor field is not resolved".into()),
                        };
                        if name != input.name()
                            || !value.is_some_and(|ty| input.matches(&project.type_store, ty))
                        {
                            return Err(
                                "primitive constructor input differs from checked ABI".into()
                            );
                        }
                        table.primitives.insert(expr, ty);
                        continue;
                    }
                    let range =
                        etas_types::value_representation::range_fields(&project.type_store, ty);
                    let names = if let Some(range) = &range {
                        if record.path.is_none() {
                            return Err("range construction requires its checked type path".into());
                        }
                        let mut names = range
                            .fields
                            .iter()
                            .map(|field| field.name.clone())
                            .collect::<Vec<_>>();
                        names.sort();
                        Some(Arc::from(names))
                    } else {
                        let state = project
                            .types
                            .agent_state_type(&project.type_store, ty)
                            .map_err(|error| error.to_string())?;
                        shapes.get(state.unwrap_or(ty))?
                    };
                    let Some(names) = names else {
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
                            range: range.is_some(),
                        },
                    );
                }
                HirExpr::Field { base, field, .. } => {
                    if project.types.bound_method_values.contains_key(&expr) {
                        continue;
                    }
                    let Some(ty) = project.types.expr_types.get(base) else {
                        continue;
                    };
                    if etas_types::value_representation::checked_value_projection(
                        &project.type_store,
                        &spec_facts,
                        *ty,
                        field,
                    )
                    .is_some()
                    {
                        let projections = project
                            .types
                            .field_projections
                            .get(&expr)
                            .ok_or("value field has no checked projection")?;
                        let [fact] = projections.as_slice() else {
                            return Err("value field requires one checked projection".into());
                        };
                        if fact.receiver != *ty
                            || fact.field != *field
                            || project.types.expression_producer_type(expr) != Some(fact.output)
                        {
                            return Err("value field differs from checked projection".into());
                        }
                        let projection = checked_value_projection(project, &spec_facts, fact)?
                            .expect("checked value projection");
                        table.value_fields.insert(expr, projection);
                        continue;
                    }
                    if Some(*ty)
                        == project
                            .types
                            .known_std_types
                            .runtime_type(etas_std::RuntimeTypeRole::CommandResult)
                    {
                        let projections = project
                            .types
                            .field_projections
                            .get(&expr)
                            .ok_or("native field has no checked projection fact")?;
                        let [projection] = projections.as_slice() else {
                            return Err("native field requires one checked projection".into());
                        };
                        if projection.receiver != *ty
                            || projection.field != *field
                            || project.types.expression_producer_type(expr)
                                != Some(projection.output)
                        {
                            return Err("native field differs from checked projection fact".into());
                        }
                        let native = native_field(project, *ty, field, projection.output)?
                            .ok_or("native field has no checked runtime descriptor")?;
                        table.native_fields.insert(expr, native);
                        continue;
                    }
                    let state = project
                        .types
                        .agent_state_type(&project.type_store, *ty)
                        .map_err(|error| error.to_string())?;
                    let receiver = state.unwrap_or(*ty);
                    let Some(names) = shapes.get(receiver)? else {
                        continue;
                    };
                    let projections = project
                        .types
                        .field_projections
                        .get(&expr)
                        .ok_or("record field has no checked projection fact")?;
                    if projections.len() != 1
                        || projections[0].field != *field
                        || projections[0].receiver != receiver
                    {
                        return Err(format!(
                            "record field `{field}` differs from checked projection fact"
                        ));
                    }
                    if !source_memory_field(project, &projections[0]) {
                        table.fields.insert(expr, checked_field(names, field)?);
                    }
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
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    let projection_count = partial.remaining.len()
                        - usize::from(project.types.bound_method_values.contains_key(&expr));
                    if projections.len() != projection_count {
                        return Err(
                            "value path length differs from checked projection facts".into()
                        );
                    }
                    let mut current = project
                        .types
                        .agent_state_type(&project.type_store, *ty)
                        .map_err(|error| error.to_string())?
                        .unwrap_or(*ty);
                    let mut fields = Vec::with_capacity(partial.remaining.len());
                    for (name, projection) in partial.remaining.iter().zip(projections) {
                        if projection.receiver != current || projection.field != *name {
                            return Err(format!(
                                "value path field `{name}` differs from checked projection facts"
                            ));
                        }
                        let field = if source_memory_field(project, projection) {
                            None
                        } else {
                            shapes
                                .get(projection.receiver)?
                                .map(|names| checked_field(names, name))
                                .transpose()?
                        };
                        current = projection.output;
                        if project.type_store.get(current).is_none() {
                            return Err("checked field projection output type is missing".into());
                        }
                        fields.push(PathField {
                            field,
                            native: native_field(project, projection.receiver, name, current)?,
                            value: checked_value_projection(project, &spec_facts, projection)?,
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

fn checked_value_projection(
    project: &CheckedProject,
    spec_facts: &etas_types::SpecFacts,
    fact: &etas_types::FieldProjectionFact,
) -> Result<Option<etas_types::value_representation::ValueProjection>, String> {
    let Some(projection) = etas_types::value_representation::checked_value_projection(
        &project.type_store,
        spec_facts,
        fact.receiver,
        &fact.field,
    ) else {
        return Ok(None);
    };
    if projection.output_type(&project.type_store, fact.receiver) != Some(fact.output) {
        return Err("value representation projection has a different checked output type".into());
    }
    Ok(Some(projection))
}

fn native_field(
    project: &CheckedProject,
    receiver: TypeId,
    name: &str,
    output: TypeId,
) -> Result<Option<etas_std::RuntimeField>, String> {
    if Some(receiver)
        != project
            .types
            .known_std_types
            .runtime_type(etas_std::RuntimeTypeRole::CommandResult)
    {
        return Ok(None);
    }
    let descriptor = etas_std::RuntimeTypeRole::CommandResult
        .fields()
        .iter()
        .find(|field| field.name == name)
        .ok_or("native field is absent from its canonical runtime type descriptor")?;
    let expected = match descriptor.ty {
        etas_std::StdPrimitiveType::I32 => etas_types::PrimitiveType::I32,
        etas_std::StdPrimitiveType::Bytes => etas_types::PrimitiveType::Bytes,
        _ => return Err("native field descriptor has an unsupported ABI type".into()),
    };
    if project.type_store.get(output) != Some(&Type::Primitive(expected)) {
        return Err("native field output differs from its canonical runtime ABI".into());
    }
    Ok(Some(*descriptor))
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
                    range: false,
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
                    let InterpValue::String(value) = value else {
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

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use etas_frontend::CheckedProject;
use etas_hir::{
    HirExpr, HirExprId, HirFieldInit, HirNodeRef, HirTreeView, ResolveResult, SymbolId,
};
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use super::{
    SlotLayoutTable,
    context::{PlanContext, plan_pass},
};

mod scopes;
#[cfg(test)]
mod tests;
use scopes::ClosureScopes;

#[derive(Clone, Debug)]
pub struct ClosureLayout {
    pub(crate) captures: Arc<[SymbolId]>,
    pub(crate) slots: Arc<SlotLayoutTable>,
}

#[derive(Clone, Debug, Default)]
pub struct ClosureLayoutTable {
    layouts: HashMap<HirExprId, ClosureLayout>,
}

impl ClosureLayoutTable {
    pub fn get(&self, expr: HirExprId) -> Option<&ClosureLayout> {
        self.layouts.get(&expr)
    }

    pub(crate) fn build(project: &CheckedProject, slots: &SlotLayoutTable) -> Result<Self, String> {
        let mut captures = HashMap::<_, BTreeSet<SymbolId>>::new();
        let mut locals = HashMap::<_, BTreeSet<SymbolId>>::new();
        for (expr, data) in project.hir.exprs.iter() {
            if matches!(data, HirExpr::Lambda { .. }) {
                captures.insert(expr, BTreeSet::new());
                locals.insert(expr, BTreeSet::new());
            }
        }
        if captures.is_empty() {
            return Ok(Self::default());
        }
        let view = HirTreeView::try_new(&project.hir)
            .map_err(|error| format!("invalid closure HIR: {error:?}"))?;
        let scope_index = ClosureScopes::build(&project.hir.scopes)?;
        let mut declarations = HashMap::new();
        for scope in project.hir.scopes.iter() {
            for symbol in scope
                .symbols
                .iter()
                .copied()
                .filter(|symbol| slots.resolve(*symbol).is_some())
            {
                if let Some(previous) = declarations.insert(symbol, scope.id)
                    && previous != scope.id
                {
                    return Err(format!(
                        "local symbol {symbol:?} has multiple declaring scopes"
                    ));
                }
                if let Some(closure) = scope_index.nearest(scope.id)? {
                    locals
                        .get_mut(&closure.expr)
                        .ok_or("lambda scope has no expression")?
                        .insert(symbol);
                }
            }
        }

        // Each resolved use contributes to every intervening closure, including an
        // outer closure that only forwards the binding to a nested closure.
        for (expr, data) in project.hir.exprs.iter() {
            let mut record_use = |resolution: &ResolveResult| -> Result<(), String> {
                let symbol = match resolution {
                    ResolveResult::Resolved(symbol) => *symbol,
                    ResolveResult::PartiallyResolved(partial) => match partial.resolved_prefix {
                        Some(symbol) => symbol,
                        None => return Ok(()),
                    },
                    _ => return Ok(()),
                };
                if slots.resolve(symbol).is_none() {
                    return Ok(());
                }
                let declaration = *declarations
                    .get(&symbol)
                    .ok_or_else(|| format!("local symbol {symbol:?} has no declaring scope"))?;
                let scope = view
                    .enclosing_scope(HirNodeRef::Expr(expr))
                    .ok_or("local use has no enclosing scope")?;
                if !scope_index.contains(declaration, scope)? {
                    return Err(format!(
                        "local symbol {symbol:?} is outside the lexical scope of {expr:?}"
                    ));
                }
                let mut nearest = scope_index.nearest(scope)?;
                while let Some(closure) = nearest {
                    if closure.scope == declaration
                        || !scope_index.contains(declaration, closure.scope)?
                    {
                        break;
                    }
                    captures
                        .get_mut(&closure.expr)
                        .ok_or("lambda scope has no expression")?
                        .insert(symbol);
                    nearest = scope_index.outer(closure)?;
                }
                Ok(())
            };
            match data {
                HirExpr::Path(path) => record_use(&path.resolution)?,
                HirExpr::Record(record) => {
                    for field in &record.fields {
                        if let HirFieldInit::Shorthand { resolution, .. } = field {
                            record_use(resolution)?;
                        }
                    }
                }
                _ => {}
            }
        }
        let layouts = captures
            .into_iter()
            .map(|(expr, captures)| {
                let own_locals = locals.remove(&expr).ok_or("missing closure locals")?;
                let symbols = own_locals
                    .into_iter()
                    .chain(captures.iter().copied())
                    .collect();
                Ok((
                    expr,
                    ClosureLayout {
                        slots: Arc::new(SlotLayoutTable::from_symbols(symbols)),
                        captures: captures.into_iter().collect::<Vec<_>>().into(),
                    },
                ))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self { layouts })
    }
}

pub(super) struct BuildClosureLayoutsPass;

impl Pass<PlanContext<'_>> for BuildClosureLayoutsPass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.build_closure_layouts")
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _: &PassContext<PlanContext<'_>>,
        _: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        let result = context
            .slots
            .as_ref()
            .ok_or_else(|| "closure planning requires slot layouts".to_owned())
            .and_then(|slots| ClosureLayoutTable::build(context.project, slots));
        match result {
            Ok(layouts) => context.closures = Some(layouts),
            Err(error) => {
                context.push_missing_fact(crate::diagnostics::primary_span(context.project), &error)
            }
        }
        PassResult::unchanged()
    }
}

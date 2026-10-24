use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use etas_frontend::CheckedProject;
use etas_hir::{HirImplItem, HirItem, ScopeId, ScopeOwner, SymbolId};
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use super::{
    SlotLayoutTable,
    context::{PlanContext, plan_pass},
};

#[derive(Clone, Debug)]
pub(crate) struct FrameLayoutTable {
    layouts: HashMap<ScopeId, Arc<SlotLayoutTable>>,
}

impl FrameLayoutTable {
    pub(crate) fn get(&self, scope: ScopeId) -> Option<&Arc<SlotLayoutTable>> {
        self.layouts.get(&scope)
    }

    fn build(project: &CheckedProject, slots: &SlotLayoutTable) -> Result<Self, String> {
        let mut layouts = HashMap::<ScopeId, Vec<SymbolId>>::new();
        let mut parameters = Vec::new();
        let mut declare = |scope, params: &[SymbolId]| -> Result<(), String> {
            if project.hir.scopes.get(scope).is_none() {
                return Err(format!("callable is missing scope {scope:?}"));
            }
            if layouts.insert(scope, Vec::new()).is_some() {
                return Err(format!("multiple callables share frame scope {scope:?}"));
            }
            parameters.extend(params.iter().map(|symbol| (scope, *symbol)));
            Ok(())
        };
        for (_, item) in project.hir.items.iter() {
            match item {
                HirItem::Flow(flow) => declare(flow.scope, &flow.params)?,
                HirItem::Tool(tool) => declare(tool.scope, &tool.params)?,
                HirItem::Agent(agent) => declare(agent.scope, &agent.params)?,
                HirItem::Impl(decl) => {
                    for member in &decl.items {
                        if let HirImplItem::Flow(flow) = member {
                            declare(flow.scope, &flow.params)?;
                        }
                    }
                }
                _ => {}
            }
        }
        for (_, handler) in project.hir.handler_arms.iter() {
            declare(handler.scope, &[])?;
        }

        // One lexical-graph walk assigns local slots to their execution frame.
        // Lambda bodies are separate frames, planned by ClosureLayoutTable.
        let count = project.hir.scopes.iter().count();
        let mut children = vec![Vec::new(); count];
        let mut pending = Vec::new();
        for scope in project.hir.scopes.iter() {
            if let Some(parent) = scope.parent {
                children
                    .get_mut(parent.index())
                    .ok_or("missing frame scope parent")?
                    .push(scope.id);
            } else {
                pending.push((scope.id, None));
            }
        }
        let mut visited = vec![false; count];
        let mut declarations = HashSet::new();
        while let Some((id, inherited)) = pending.pop() {
            let seen = visited
                .get_mut(id.index())
                .ok_or("invalid frame scope identity")?;
            if *seen {
                return Err("repeated frame scope traversal".into());
            }
            *seen = true;
            let scope = project.hir.scopes.get(id).ok_or("missing frame scope")?;
            let owner = if layouts.contains_key(&id) {
                Some(id)
            } else if matches!(scope.owner, ScopeOwner::Lambda(_)) {
                None
            } else {
                inherited
            };
            for symbol in scope
                .symbols
                .iter()
                .copied()
                .filter(|symbol| slots.resolve(*symbol).is_some())
            {
                if !declarations.insert(symbol) {
                    return Err(format!(
                        "local symbol {symbol:?} has multiple declaring scopes"
                    ));
                }
                if let Some(owner) = owner {
                    layouts
                        .get_mut(&owner)
                        .ok_or("missing owning frame layout")?
                        .push(symbol);
                }
            }
            pending.extend(children[id.index()].iter().map(|child| (*child, owner)));
        }
        if visited.iter().any(|seen| !seen) {
            return Err("cyclic frame scope ancestry".into());
        }
        let layouts: HashMap<_, _> = layouts
            .into_iter()
            .map(|(scope, symbols)| (scope, Arc::new(SlotLayoutTable::from_symbols(symbols))))
            .collect();
        for (scope, symbol) in parameters {
            if layouts
                .get(&scope)
                .and_then(|layout| layout.resolve(symbol))
                .is_none()
            {
                return Err(format!(
                    "callable parameter {symbol:?} is outside frame scope {scope:?}"
                ));
            }
        }
        Ok(Self { layouts })
    }
}

pub(super) struct BuildFrameLayoutsPass;

impl Pass<PlanContext<'_>> for BuildFrameLayoutsPass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.build_frame_layouts")
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
            .ok_or_else(|| "frame planning requires slot layouts".to_owned())
            .and_then(|slots| FrameLayoutTable::build(context.project, slots));
        match result {
            Ok(layouts) => context.frames = Some(layouts),
            Err(error) => {
                context.push_missing_fact(crate::diagnostics::primary_span(context.project), &error)
            }
        }
        PassResult::unchanged()
    }
}

#[cfg(test)]
mod tests;

use std::{collections::HashMap, sync::Arc};

use etas_frontend::CheckedProject;
#[cfg(test)]
use etas_hir::{HirExpr, ResolveResult};
use etas_hir::{HirExprId, SymbolId};
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use super::{
    SlotLayoutTable,
    context::{PlanContext, plan_pass},
};

mod scopes;
#[cfg(test)]
mod tests;

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

    pub(crate) fn build(project: &CheckedProject) -> Result<Self, String> {
        let layouts = etas_hir_analysis::closures::analyze_closures(&project.hir.view())?
            .into_iter()
            .map(|(expr, layout)| {
                let symbols = layout
                    .locals
                    .into_iter()
                    .chain(layout.captures.iter().copied())
                    .collect();
                (
                    expr,
                    ClosureLayout {
                        slots: Arc::new(SlotLayoutTable::from_symbols(symbols)),
                        captures: layout.captures.into(),
                    },
                )
            })
            .collect();
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
        let result = ClosureLayoutTable::build(context.project);
        match result {
            Ok(layouts) => context.closures = Some(layouts),
            Err(error) => {
                context.push_missing_fact(crate::diagnostics::primary_span(context.project), &error)
            }
        }
        PassResult::unchanged()
    }
}

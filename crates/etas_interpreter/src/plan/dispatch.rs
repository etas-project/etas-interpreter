use std::collections::{HashMap, HashSet};

use etas_builtin::PureIntrinsicRegistry;
use etas_hir::HirExprId;
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use super::context::{PlanContext, plan_pass};

#[derive(Clone, Debug, Default)]
pub struct IntrinsicDispatchTable {
    pub pure_registry: PureIntrinsicRegistry,
    map_exprs: HashSet<HirExprId>,
    brace_literals: HashMap<HirExprId, BraceLiteralShape>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BraceLiteralShape {
    Record,
    Map,
}

impl IntrinsicDispatchTable {
    pub fn with_expr_shapes(
        map_exprs: HashSet<HirExprId>,
        brace_literals: HashMap<HirExprId, BraceLiteralShape>,
    ) -> Self {
        Self {
            pure_registry: PureIntrinsicRegistry,
            map_exprs,
            brace_literals,
        }
    }

    pub fn is_map_expr(&self, expr: HirExprId) -> bool {
        self.map_exprs.contains(&expr)
    }

    pub fn brace_literal_shape(&self, expr: HirExprId) -> Option<BraceLiteralShape> {
        self.brace_literals.get(&expr).copied()
    }
}

pub(super) struct BuildIntrinsicDispatchTablePass;

impl Pass<PlanContext<'_>> for BuildIntrinsicDispatchTablePass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.build_intrinsic_dispatch")
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _pass_context: &PassContext<PlanContext<'_>>,
        _manager: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        let mut map_exprs = HashSet::new();
        let mut brace_literals = HashMap::new();
        for (expr, ty) in context.project.types.expr_types.iter() {
            match context.project.type_store.get(*ty) {
                Some(etas_types::Type::Record(_)) => {
                    brace_literals.insert(*expr, BraceLiteralShape::Record);
                }
                Some(etas_types::Type::Map { .. }) => {
                    map_exprs.insert(*expr);
                    brace_literals.insert(*expr, BraceLiteralShape::Map);
                }
                _ => {}
            }
        }
        context.dispatch = Some(IntrinsicDispatchTable::with_expr_shapes(
            map_exprs,
            brace_literals,
        ));
        PassResult::unchanged()
    }
}

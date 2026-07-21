use etas_hir::{SymbolDef, SymbolId, TopLevelLetClassification};
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use super::context::{PlanContext, plan_pass};

#[derive(Clone, Debug, Default)]
pub struct GlobalTable {
    pub constants: Vec<GlobalConst>,
}

#[derive(Clone, Debug)]
pub struct GlobalConst {
    pub symbol: SymbolId,
}

pub(super) struct BuildGlobalTablePass;

impl Pass<PlanContext<'_>> for BuildGlobalTablePass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.build_globals")
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _pass_context: &PassContext<PlanContext<'_>>,
        _manager: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        let constants = context
            .project
            .symbols
            .iter()
            .filter_map(|symbol| match &symbol.def {
                SymbolDef::TopLevelLet {
                    classification: TopLevelLetClassification::Const,
                    ..
                } => Some(GlobalConst { symbol: symbol.id }),
                _ => None,
            })
            .collect();
        context.globals = Some(GlobalTable { constants });
        PassResult::unchanged()
    }
}

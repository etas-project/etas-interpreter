use etas_hir::{SymbolDef, SymbolId, TopLevelLetClassification};
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use super::context::{PlanContext, plan_pass};

#[derive(Clone, Debug, Default)]
pub struct ResourceTable {
    pub handles: Vec<ResourceHandleDescriptor>,
}

#[derive(Clone, Debug)]
pub struct ResourceHandleDescriptor {
    pub symbol: SymbolId,
}

pub(super) struct BuildResourceHandleTablePass;

impl Pass<PlanContext<'_>> for BuildResourceHandleTablePass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.build_resource_handles")
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _pass_context: &PassContext<PlanContext<'_>>,
        _manager: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        let handles = context
            .project
            .symbols
            .iter()
            .filter_map(|symbol| match &symbol.def {
                SymbolDef::TopLevelLet {
                    classification: TopLevelLetClassification::ResourceHandle(_),
                    ..
                } => Some(ResourceHandleDescriptor { symbol: symbol.id }),
                _ => None,
            })
            .collect();
        context.resources = Some(ResourceTable { handles });
        PassResult::unchanged()
    }
}

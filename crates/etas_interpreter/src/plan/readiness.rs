use etas_effects::InterpreterSupport;
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use crate::diagnostics::item_span;

use super::context::{PlanContext, plan_pass};

#[derive(Clone, Debug, Default)]
pub struct HostRequirementTable {
    pub entry: Option<InterpreterSupport>,
}

pub(super) struct ComputeReachableHostRequirementsPass;

impl Pass<PlanContext<'_>> for ComputeReachableHostRequirementsPass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.compute_host_requirements")
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _pass_context: &PassContext<PlanContext<'_>>,
        _manager: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        let Some(entry) = context.entry.map(|entry| entry.item) else {
            return PassResult::unchanged();
        };
        let entry_support = context
            .project
            .interpreter_support
            .entry
            .clone()
            .or_else(|| {
                context
                    .project
                    .interpreter_support
                    .items
                    .get(&entry)
                    .cloned()
            });
        if entry_support.is_none() {
            context.push_missing_fact(
                item_span(context.project, entry),
                "checked project is missing interpreter-support facts for the selected entry",
            );
        }
        context.host_requirements = Some(HostRequirementTable {
            entry: entry_support,
        });
        PassResult::unchanged()
    }
}

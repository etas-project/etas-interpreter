use etas_utils::{Pass, PassContext, PassManager, PassResult};

use crate::diagnostics;

use super::context::{PlanContext, plan_pass};

pub(super) struct ValidateCheckedProjectPass;

impl Pass<PlanContext<'_>> for ValidateCheckedProjectPass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.validate_checked_project")
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _pass_context: &PassContext<PlanContext<'_>>,
        _manager: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        if context.project.interpreter_support.items.is_empty()
            && context.project.interpreter_support.entry.is_none()
        {
            context.push_missing_fact(
                diagnostics::primary_span(context.project),
                "checked project does not contain interpreter-support facts",
            );
        }
        PassResult::unchanged()
    }
}

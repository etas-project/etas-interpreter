use etas_utils::{ArtifactSet, Pass, PassContext, PassManager, PassResult};

use crate::{api::EntryPoint, diagnostics};

use super::context::{PlanContext, plan_pass};
use super::residual::artifacts::{SELECTED_CALLABLE, produced};

pub(super) struct BuildEntryPlanPass;

impl Pass<PlanContext<'_>> for BuildEntryPlanPass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.build_entry").produces(ArtifactSet::one(SELECTED_CALLABLE))
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _pass_context: &PassContext<PlanContext<'_>>,
        _manager: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        context.entry = None;
        let Some(root) = context.requested_root else {
            context.diagnostics.push(diagnostics::missing_entry(
                diagnostics::primary_span(context.project),
                "checked project does not contain a resolved entry item",
            ));
            return PassResult::failed("callable selection has no checked root");
        };
        match context.project.checked_callable_owner(root) {
            Ok(item) => {
                context.entry = Some(EntryPoint { item });
                produced(SELECTED_CALLABLE)
            }
            Err(error) => {
                context.push_missing_fact(
                    diagnostics::primary_span(context.project),
                    &error.to_string(),
                );
                PassResult::failed(error.to_string())
            }
        }
    }
}

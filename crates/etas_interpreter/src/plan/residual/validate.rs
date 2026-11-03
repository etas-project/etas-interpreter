use std::collections::BTreeSet;

use etas_code::residual::target::validate_backend_support;
use etas_utils::{ArtifactSet, Pass, PassContext, PassManager, PassResult};

use crate::diagnostics::{item_span, primary_span};
use crate::plan::context::{PlanContext, plan_pass};

use super::{ValidatedResidualPlans, artifacts::*, interpreter_residual_capabilities};

/// Revalidates the frontend's residual plans against this engine before any
/// site is bound: the plans must have been checked for exactly this engine's
/// capabilities, every plan must be natively supported, the final revision
/// binding must validate, and every plan obligation must be one of its
/// owner's checked residual references.
pub(in crate::plan) struct ValidateResidualPlansPass;

impl Pass<PlanContext<'_>> for ValidateResidualPlansPass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.validate_residual_plans")
            .produces(ArtifactSet::one(VALIDATED_RESIDUAL_PLANS))
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _pass_context: &PassContext<PlanContext<'_>>,
        _manager: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        context.validated_residual = None;
        context.residual = None;
        let project = context.project;
        let plans = project.residual_plans();
        if plans.is_empty() {
            context.validated_residual = Some(ValidatedResidualPlans {
                program: project.clone(),
            });
            return produced(VALIDATED_RESIDUAL_PLANS);
        }
        let span = primary_span(project);
        let capabilities = interpreter_residual_capabilities();
        if plans.target() != &capabilities {
            context.push_missing_fact(
                span,
                "residual plans were checked against a different execution target",
            );
            return PassResult::failed("residual plan validation failed");
        }
        let Some(binding) = context.residual_binding.as_ref() else {
            context.push_missing_fact(span, "residual plans have no final revision binding");
            return PassResult::failed("residual plan validation failed");
        };
        if let Err(error) = binding.validate(plans) {
            context.push_missing_fact(
                span,
                &format!("residual plan revision binding is invalid: {error:?}"),
            );
            return PassResult::failed("residual plan validation failed");
        }
        for plan in plans.plans() {
            let span = item_span(project, plan.item());
            match validate_backend_support(plan.plan(), &capabilities) {
                Ok(support) if support == plan.support() => {}
                Ok(_) => {
                    context.push_missing_fact(
                        span,
                        "residual plan support was checked for a different plan or target",
                    );
                    return PassResult::failed("residual plan validation failed");
                }
                Err(error) => {
                    context.push_missing_fact(
                        span,
                        &format!("residual plan is not supported by this interpreter: {error:?}"),
                    );
                    return PassResult::failed("residual plan validation failed");
                }
            }
            let Some(summary) = project.effects.item_effects.get(&plan.item()) else {
                context.push_missing_fact(span, "residual plan owner has no checked effects");
                return PassResult::failed("residual plan validation failed");
            };
            let checked = summary.residual_checks.iter().collect::<BTreeSet<_>>();
            if plan
                .references()
                .iter()
                .any(|reference| !checked.contains(reference))
            {
                context.push_missing_fact(
                    span,
                    "residual plan obligations are not checked references of their owner",
                );
                return PassResult::failed("residual plan validation failed");
            }
        }
        context.validated_residual = Some(ValidatedResidualPlans {
            program: project.clone(),
        });
        produced(VALIDATED_RESIDUAL_PLANS)
    }
}

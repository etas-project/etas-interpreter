use std::collections::BTreeMap;
use std::sync::Arc;

use etas_code::residual::plan::SemanticPlanComponent;
use etas_hir::HirItem;
use etas_utils::{ArtifactSet, Pass, PassContext, PassManager, PassResult};

use crate::diagnostics::item_span;
use crate::plan::context::{PlanContext, plan_pass};

use super::{
    PreparedResidualPlans, PreparedResidualScope, PreparedTracePlan, artifacts::*,
    interpreter_residual_capabilities,
};

/// Binds every validated plan to the checked callable whose invocations
/// activate it. A plan must have one scope and activation shared by all of
/// its obligations, only trace components, and a final bound plan; its owner
/// must be a callable whose body entry activates scopes.
pub(in crate::plan) struct BindResidualSitesPass;

impl Pass<PlanContext<'_>> for BindResidualSitesPass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.bind_residual_sites")
            .requires(ArtifactSet::one(VALIDATED_RESIDUAL_PLANS))
            .produces(ArtifactSet::one(BOUND_RESIDUAL_SITES))
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _pass_context: &PassContext<PlanContext<'_>>,
        _manager: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        context.residual = None;
        let Some(validated) = context.validated_residual.take() else {
            return PassResult::failed("residual binding lost its validated plan artifact");
        };
        if !Arc::ptr_eq(&validated.program, context.project) {
            return PassResult::failed("residual validation belongs to another checked program");
        }
        let project = context.project;
        let plans = project.residual_plans();
        if plans.is_empty() {
            context.residual = Some(PreparedResidualPlans::default());
            return produced(BOUND_RESIDUAL_SITES);
        }
        let Some(binding) = context.residual_binding.as_ref() else {
            return PassResult::failed("validated residual plans lost their revision binding");
        };
        let mut scopes = BTreeMap::new();
        for item in plans.items() {
            let span = item_span(project, item);
            if !matches!(
                project.hir.items.get(item),
                Some(HirItem::Flow(_) | HirItem::Tool(_) | HirItem::Agent(_))
            ) {
                context.push_missing_fact(
                    span,
                    "residual plan owner is not a callable whose invocation activates a scope",
                );
                return PassResult::failed("residual site binding failed");
            }
            let Some(revision) = binding.revision(item) else {
                context.push_missing_fact(span, "residual plan owner has no checked revision");
                return PassResult::failed("residual site binding failed");
            };
            let mut prepared = Vec::new();
            for plan in plans.item_plans(item) {
                let Some(bound) = binding.bound(plan.type_fact_index()) else {
                    context.push_missing_fact(span, "residual plan has no final binding");
                    return PassResult::failed("residual site binding failed");
                };
                let mut site = None;
                for obligation in plan.plan().obligations().iter() {
                    let identity = obligation.identity();
                    let current = (identity.scope, identity.activation);
                    if site.is_some_and(|site| site != current) {
                        context.push_missing_fact(
                            span,
                            "residual plan obligations do not share one invocation scope",
                        );
                        return PassResult::failed("residual site binding failed");
                    }
                    site = Some(current);
                    if !matches!(
                        plan.plan().component_for(obligation.id()),
                        Some(SemanticPlanComponent::Trace(_))
                    ) {
                        context.push_missing_fact(
                            span,
                            "residual plan component kind is not executable by this interpreter",
                        );
                        return PassResult::failed("residual site binding failed");
                    }
                }
                let Some((scope, activation)) = site else {
                    context.push_missing_fact(span, "residual plan has no obligations");
                    return PassResult::failed("residual site binding failed");
                };
                prepared.push(PreparedTracePlan {
                    checked: plan.clone(),
                    scope,
                    activation,
                    bound: bound.data().fingerprint,
                });
            }
            let Some(summary) = project.effects.item_effects.get(&item) else {
                return PassResult::failed("residual owner has no checked action footprint");
            };
            let applications = match plans.applications(project, item) {
                Ok(applications) => applications.into_iter().map(Arc::new).collect::<Vec<_>>(),
                Err(error) => {
                    context.push_missing_fact(
                        span,
                        &format!("residual application binding failed: {error:?}"),
                    );
                    return PassResult::failed("residual site binding failed");
                }
            };
            let mut approval_target = None;
            for effect in summary
                .requested_actions
                .effects
                .iter()
                .chain(summary.default_actions.effects.iter())
            {
                if !effect.is_action() {
                    continue;
                }
                if crate::control::residual::is_standard_approval(&project.effect_registry, effect)
                {
                    let label = match plans.canonical_action(project, item, effect) {
                        Ok(label) => label,
                        Err(error) => {
                            return PassResult::failed(format!(
                                "approval action identity: {error:?}"
                            ));
                        }
                    };
                    approval_target = Some(label.target);
                }
            }
            scopes.insert(
                item,
                Arc::new(PreparedResidualScope {
                    item,
                    revision: revision.key(),
                    plans: prepared,
                    applications,
                    approval_target,
                }),
            );
        }
        context.residual = Some(PreparedResidualPlans {
            target: Some(interpreter_residual_capabilities().fingerprint()),
            scopes,
        });
        produced(BOUND_RESIDUAL_SITES)
    }
}

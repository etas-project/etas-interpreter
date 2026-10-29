use super::*;
pub(crate) use crate::value::ordered::default_order;

/// Purity is checked against the executable target, not merely its escaping row:
/// default-handled actions and checkpoint orchestration are also forbidden.
pub(crate) fn validate_comparator(
    checked: &CheckedProject,
    plan: &InterpreterPlan,
    target: &CallTarget,
) -> Result<(), String> {
    let mut pending = vec![(
        target.clone(),
        None::<crate::value::CheckedExecutionProgram>,
    )];
    let mut visited = std::collections::HashSet::new();
    while let Some((target, owner)) = pending.pop() {
        let checked = owner.as_ref().map_or(checked, |owner| &owner.checked);
        let plan = owner.as_ref().map_or(plan, |owner| owner.plan.as_ref());
        let root = match target {
            CallTarget::FlowItem(item) | CallTarget::ToolItem(item) => {
                etas_frontend::CheckedCallableRoot::Item(item)
            }
            CallTarget::Lambda { expr, .. } => etas_frontend::CheckedCallableRoot::Lambda(expr),
            CallTarget::PureIntrinsic(call) => plan.dispatch.intrinsic_reference_root(
                call.intrinsic,
                &call.parameter_types,
                call.result_type,
            )?,
            CallTarget::StdIntrinsic(call) => plan.dispatch.std_callable_root(&call)?,
            CallTarget::Scoped(value) => {
                pending.push((value.target(), Some(value.program().clone())));
                continue;
            }
            CallTarget::Specialized { target, .. } | CallTarget::Limited { target, .. } => {
                if target.shared_identity().is_none_or(|key| {
                    visited.insert((std::sync::Arc::as_ptr(checked) as usize, 0, key as usize))
                }) {
                    pending.push(((*target).clone(), owner));
                }
                continue;
            }
            CallTarget::Composed(children) => {
                if children.shared_identity().is_some_and(|key| {
                    !visited.insert((std::sync::Arc::as_ptr(checked) as usize, 1, key as usize))
                }) {
                    continue;
                }
                for child in children.iter() {
                    pending.push((child.clone(), owner.clone()));
                }
                continue;
            }
            _ => return Err("collection comparison requires a checked pure flow".into()),
        };
        let summary = checked
            .checked_callable_effects(root)
            .map_err(|error| error.to_string())?;
        validate_pure_summary(summary)?;
    }
    Ok(())
}

pub(crate) fn validate_pure_summary(summary: &etas_effects::EffectSummary) -> Result<(), String> {
    if summary.escaping_effects.is_empty()
        && summary.requested_actions.is_empty()
        && summary.default_actions.is_empty()
        && summary
            .requirements
            .iter()
            .all(|requirement| matches!(requirement, etas_effects::RequirementFact::Limit(_)))
        && summary.trace_spec_obligations.iter().next().is_none()
        && summary.residual_checks.checks.is_empty()
        && summary.determinism == etas_effects::Determinism::Deterministic
        && summary.support == etas_effects::InterpreterSupport::LocalOnly
    {
        Ok(())
    } else {
        Err("collection comparison must be pure: host actions, open effects and nonlocal execution are not allowed".into())
    }
}

impl EvalContext<'_> {
    pub(super) fn validate_collection_comparator(&self, target: &CallTarget) -> Result<(), String> {
        validate_comparator(self.checked, self.plan, target)
    }
}

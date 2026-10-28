use etas_effects::InterpreterSupport;
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use crate::diagnostics::item_span;

use super::context::{PlanContext, plan_pass};

mod dispatch;

#[derive(Clone, Debug, Default)]
pub struct HostRequirementTable {
    pub entry: Option<InterpreterSupport>,
    pub(crate) service_check: HostServiceCheck,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum HostServiceCheck {
    #[default]
    Preflight,
    EscapedBoundary,
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
        if context
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
        {
            return PassResult::unchanged();
        }
        let Some(entry) = context.entry.map(|entry| entry.item) else {
            return PassResult::unchanged();
        };
        let Some(root) = context.requested_root else {
            return PassResult::unchanged();
        };
        let entry_support = match context.project.checked_callable_effects(root) {
            Ok(summary) => match root {
                etas_frontend::CheckedCallableRoot::Lambda(_)
                | etas_frontend::CheckedCallableRoot::StdReference(_) => {
                    Some(summary.support.clone())
                }
                etas_frontend::CheckedCallableRoot::Item(item) => {
                    match context.project.effects.interpreter_support.items.get(&item) {
                        Some(support) if support == &summary.support => Some(support.clone()),
                        _ => {
                            context.push_missing_fact(item_span(context.project, entry),
                                "selected callable has missing or inconsistent interpreter-support facts");
                            None
                        }
                    }
                }
            },
            Err(error) => {
                context.push_missing_fact(item_span(context.project, entry), &error.to_string());
                None
            }
        };
        let service_check = match dispatch::service_check(context.project, root) {
            Ok(mode) => mode,
            Err(reason) => {
                context.push_missing_fact(item_span(context.project, entry), &reason);
                return PassResult::unchanged();
            }
        };
        context.host_requirements = Some(HostRequirementTable {
            entry: entry_support,
            service_check,
        });
        PassResult::unchanged()
    }
}

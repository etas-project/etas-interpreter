use std::cmp::Ordering;

use super::*;
use crate::value::priority_queue::{Operation, Update, UpdateStep};

#[derive(Clone, Debug)]
pub struct PriorityQueueUpdate {
    update: Update,
    span: Span,
    awaiting_call: bool,
    order: Option<Ordering>,
}

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
            CallTarget::SpecImplMethod(symbol) => {
                let Some(SymbolDef::Item { item }) =
                    checked.symbols.get(symbol).map(|symbol| &symbol.def)
                else {
                    return Err("priority comparison method has no checked item".into());
                };
                etas_frontend::CheckedCallableRoot::Item(*item)
            }
            _ => return Err("priority comparison requires a checked pure flow".into()),
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
        Err("priority comparison must be pure: host actions, open effects and nonlocal execution are not allowed".into())
    }
}

impl EvalContext<'_> {
    pub(super) fn validate_priority_comparator(&self, target: &CallTarget) -> Result<(), String> {
        validate_comparator(self.checked, self.plan, target)
    }

    pub(super) fn start_priority_update(
        &mut self,
        queue: crate::value::PriorityQueueValue,
        operation: Operation,
        span: Span,
    ) -> ControlSignal {
        self.continue_priority_update(
            PriorityQueueUpdate {
                update: queue.update(operation),
                span,
                awaiting_call: false,
                order: None,
            },
            InterpValue::Unit,
        )
    }

    pub(crate) fn continue_priority_update(
        &mut self,
        mut state: PriorityQueueUpdate,
        value: InterpValue,
    ) -> ControlSignal {
        let order = if state.awaiting_call {
            let InterpValue::Number(crate::value::NumericValue::I32(value)) = value else {
                return ControlSignal::missing_checked_fact(
                    "priority comparison did not return i32",
                    state.span,
                );
            };
            state.awaiting_call = false;
            Some(value.cmp(&0))
        } else {
            if value != InterpValue::Unit {
                return ControlSignal::missing_checked_fact(
                    "unexpected priority update continuation input",
                    state.span,
                );
            }
            state.order.take()
        };
        match state.update.advance(order) {
            Err(error) => ControlSignal::missing_checked_fact(error, state.span),
            Ok(UpdateStep::Complete) => {
                let result = match state.update.finish() {
                    Ok(result) => result,
                    Err(message) => {
                        return ControlSignal::missing_checked_fact(message, state.span);
                    }
                };
                let queue = InterpValue::PriorityQueue(result.queue);
                ControlSignal::Value(if result.was_pop {
                    InterpValue::Tuple(
                        vec![
                            queue,
                            result.popped.map_or(InterpValue::OptionNone, |v| {
                                InterpValue::OptionSome(v.into())
                            }),
                        ]
                        .into(),
                    )
                } else {
                    queue
                })
            }
            Ok(UpdateStep::Continue) => ControlSignal::pending_continuation(PendingContinuation {
                continuation: Continuation::PriorityQueueUpdate(Box::new(state)),
                input: ContinuationInput::Value(InterpValue::Unit),
            }),
            Ok(UpdateStep::Compare) => {
                let comparison = match state.update.comparison() {
                    Ok(comparison) => comparison,
                    Err(message) => {
                        return ControlSignal::missing_checked_fact(message, state.span);
                    }
                };
                if let Some(target) = comparison.target {
                    state.awaiting_call = true;
                    return ControlSignal::pending_call(PendingCall {
                        site: None,
                        target,
                        args: vec![comparison.left, comparison.right],
                        span: state.span,
                        continuation: Continuation::PriorityQueueUpdate(Box::new(state)),
                    });
                }
                state.order = match default_order(&comparison.left, &comparison.right) {
                    Ok(order) => Some(order),
                    Err(message) => return ControlSignal::invalid_arguments(message, state.span),
                };
                ControlSignal::pending_continuation(PendingContinuation {
                    continuation: Continuation::PriorityQueueUpdate(Box::new(state)),
                    input: ContinuationInput::Value(InterpValue::Unit),
                })
            }
        }
    }
}

fn default_order(left: &InterpValue, right: &InterpValue) -> Result<Ordering, String> {
    let order = match (left, right) {
        (InterpValue::Number(left), InterpValue::Number(right)) => left
            .partial_cmp_same(*right)
            .map_err(|error| format!("invalid priority comparison: {error:?}"))?,
        (InterpValue::String(left), InterpValue::String(right)) => Some(left.cmp(right)),
        (InterpValue::Bool(left), InterpValue::Bool(right)) => Some(left.cmp(right)),
        _ => None,
    };
    order.ok_or_else(|| {
        "priority values have no default total order; provide a pure comparison flow".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_comparator_purity_validation_handles_shared_callable_dags() {
        let checked = crate::testing::project::checked_project(
            "module app.main; flow identity(value: i32) -> i32 ![] { return value; } flow main() -> unit { return; }",
        );
        let plan = crate::Interpreter
            .plan(&checked, crate::api::PlanOptions)
            .plan
            .unwrap();
        let item = checked
            .hir
            .items
            .iter()
            .find_map(|(item, decl)| match decl {
                etas_hir::HirItem::Flow(flow)
                    if checked.symbols.get(flow.symbol).unwrap().name == "identity" =>
                {
                    Some(item)
                }
                _ => None,
            })
            .unwrap();
        let mut target = CallTarget::FlowItem(item);
        for _ in 0..4000 {
            let link: crate::control::CallTargetLink = target.into();
            target = CallTarget::Composed(
                vec![
                    CallTarget::Limited {
                        target: link.clone(),
                        limits: vec![],
                    },
                    CallTarget::Limited {
                        target: link,
                        limits: vec![],
                    },
                ]
                .into(),
            );
        }
        validate_comparator(&checked, &plan, &target).unwrap();
    }
}

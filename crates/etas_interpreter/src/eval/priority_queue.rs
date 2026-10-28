use std::cmp::Ordering;

use super::collection_ordering::default_order;
#[cfg(test)]
use super::collection_ordering::validate_comparator;
use super::*;
use crate::value::priority_queue::{Operation, Update, UpdateStep};

#[derive(Clone, Debug)]
pub struct PriorityQueueUpdate {
    update: Update,
    span: Span,
    awaiting_call: bool,
    order: Option<Ordering>,
}

impl EvalContext<'_> {
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
                        purity: crate::control::CallPurity::Pure,
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

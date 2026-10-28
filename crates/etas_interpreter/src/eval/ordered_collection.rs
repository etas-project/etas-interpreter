use super::*;
use crate::value::ordered::{Search, SearchStep};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug)]
pub(crate) enum OrderedResult {
    Map,
    Set,
    Get,
    Contains,
}

#[derive(Clone, Debug)]
pub struct OrderedSearch {
    search: Search,
    comparator: Option<CallTarget>,
    result: OrderedResult,
    span: Span,
    awaiting_call: bool,
    order: Option<Ordering>,
}

impl EvalContext<'_> {
    pub(super) fn start_ordered_search(
        &mut self,
        search: Search,
        comparator: Option<CallTarget>,
        result: OrderedResult,
        span: Span,
    ) -> ControlSignal {
        if comparator.is_none()
            && let Err(message) =
                super::collection_ordering::default_order(search.key(), search.key())
        {
            return ControlSignal::invalid_arguments(message, span);
        }
        self.continue_ordered_search(
            OrderedSearch {
                search,
                comparator,
                result,
                span,
                awaiting_call: false,
                order: None,
            },
            InterpValue::Unit,
        )
    }

    pub(crate) fn continue_ordered_search(
        &mut self,
        mut state: OrderedSearch,
        value: InterpValue,
    ) -> ControlSignal {
        let order = if state.awaiting_call {
            let InterpValue::Number(crate::value::NumericValue::I32(value)) = value else {
                return ControlSignal::missing_checked_fact(
                    "collection comparison did not return i32",
                    state.span,
                );
            };
            state.awaiting_call = false;
            Some(value.cmp(&0))
        } else {
            if value != InterpValue::Unit {
                return ControlSignal::missing_checked_fact(
                    "unexpected ordered search continuation input",
                    state.span,
                );
            }
            state.order.take()
        };
        match state.search.advance(order) {
            Err(message) => ControlSignal::missing_checked_fact(message, state.span),
            Ok(SearchStep::Complete) => {
                let result = match state.search.finish() {
                    Ok(result) => result,
                    Err(message) => {
                        return ControlSignal::missing_checked_fact(message, state.span);
                    }
                };
                ControlSignal::Value(match state.result {
                    OrderedResult::Map => InterpValue::OrderedMap(
                        crate::value::OrderedMapValue::from_tree(result.tree, state.comparator),
                    ),
                    OrderedResult::Set => InterpValue::OrderedSet(
                        crate::value::OrderedSetValue::from_tree(result.tree, state.comparator),
                    ),
                    OrderedResult::Get => result.found.map_or(InterpValue::OptionNone, |value| {
                        InterpValue::OptionSome(value.into())
                    }),
                    OrderedResult::Contains => InterpValue::Bool(result.found.is_some()),
                })
            }
            Ok(SearchStep::Continue) => ControlSignal::pending_continuation(PendingContinuation {
                continuation: Continuation::OrderedSearch(Box::new(state)),
                input: ContinuationInput::Value(InterpValue::Unit),
            }),
            Ok(SearchStep::Compare) => {
                let (left, right) = match state.search.comparison() {
                    Ok((left, right)) => (left.clone(), right.clone()),
                    Err(message) => {
                        return ControlSignal::missing_checked_fact(message, state.span);
                    }
                };
                if let Some(target) = state.comparator.clone() {
                    state.awaiting_call = true;
                    return ControlSignal::pending_call(PendingCall {
                        purity: crate::control::CallPurity::Pure,
                        site: None,
                        target,
                        args: vec![left, right],
                        span: state.span,
                        continuation: Continuation::OrderedSearch(Box::new(state)),
                    });
                }
                state.order = match super::collection_ordering::default_order(&left, &right) {
                    Ok(order) => Some(order),
                    Err(message) => return ControlSignal::invalid_arguments(message, state.span),
                };
                ControlSignal::pending_continuation(PendingContinuation {
                    continuation: Continuation::OrderedSearch(Box::new(state)),
                    input: ContinuationInput::Value(InterpValue::Unit),
                })
            }
        }
    }
}

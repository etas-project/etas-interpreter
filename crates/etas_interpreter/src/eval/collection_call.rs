use super::*;
use crate::intrinsic::dispatch::CheckedStdIntrinsicCall;
use etas_std::{StdCollectionIntrinsic, StdCollectionKind as K, StdCollectionOperation as O};

impl EvalContext<'_> {
    pub(super) fn execute_collection_intrinsic(
        &mut self,
        operation: StdCollectionIntrinsic,
        checked: &CheckedStdIntrinsicCall,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        if checked.identity.intrinsic != operation.id()
            || args.len() != checked.parameter_types.len()
        {
            return ControlSignal::missing_checked_fact(
                "collection intrinsic ABI differs from its checked signature",
                span,
            );
        }
        match operation.operation {
            O::New | O::WithCompare => {
                return self.build_advanced_collection(operation.kind, &args, span);
            }
            O::Closed | O::Open => {
                let bounds = if operation.operation == O::Closed {
                    crate::value::RangeBounds::ClosedClosed
                } else {
                    crate::value::RangeBounds::OpenOpen
                };
                return self.build_range(bounds, &args, span);
            }
            _ => (),
        }
        let mut args = args.into_iter();
        let Some(receiver) = args.next() else {
            return ControlSignal::missing_checked_fact(
                "collection intrinsic is missing its checked receiver",
                span,
            );
        };
        if !matches!(
            (operation.kind, &receiver),
            (K::Array, InterpValue::Array(_))
                | (K::List, InterpValue::List(_))
                | (K::Slice, InterpValue::Slice(_))
                | (K::Map, InterpValue::Map(_))
                | (K::Deque, InterpValue::Deque(_))
                | (K::Queue, InterpValue::Queue(_))
                | (K::Stack, InterpValue::Stack(_))
                | (K::PriorityQueue, InterpValue::PriorityQueue(_))
                | (K::OrderedMap, InterpValue::OrderedMap(_))
                | (K::OrderedSet, InterpValue::OrderedSet(_))
        ) {
            return ControlSignal::missing_checked_fact(
                "collection intrinsic receiver violates its checked ABI",
                span,
            );
        }
        if operation.operation == O::At {
            let Some(error) = self.checked.types.known_std_types.index_error else {
                return ControlSignal::missing_checked_fact(
                    "collection at intrinsic requires the checked IndexError type",
                    span,
                );
            };
            // The callable signature, not an operator HIR node, proves this ABI.
            return match self.eval_local_method_with_values(
                checked.origin,
                receiver,
                "get",
                &[],
                args.collect(),
                span,
            ) {
                ControlSignal::Value(InterpValue::OptionSome(value)) => {
                    ControlSignal::Value(value.into_value())
                }
                ControlSignal::Value(InterpValue::OptionNone) => self.raise_typed_index_error(
                    error,
                    "sequence index is invalid or out of bounds",
                    span,
                ),
                ControlSignal::Value(_) => ControlSignal::missing_checked_fact(
                    "checked sequence lookup returned an inconsistent Option ABI",
                    span,
                ),
                other => other,
            };
        }
        self.eval_local_method_with_values(
            checked.origin,
            receiver,
            operation.method_name(),
            &[],
            args.collect(),
            span,
        )
    }
}

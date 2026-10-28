use super::*;

impl<'a> EvalContext<'a> {
    pub(in crate::eval) fn build_range(
        &mut self,
        bounds: crate::value::RangeBounds,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        let [start, end] = args else {
            return ControlSignal::invalid_arguments(
                "Range constructor expects exactly two arguments",
                span,
            );
        };
        ControlSignal::Value(InterpValue::Range(crate::value::RangeValue {
            start: Box::new(start.clone()),
            end: Box::new(end.clone()),
            bounds,
        }))
    }

    pub(in crate::eval) fn build_advanced_collection(
        &mut self,
        collection: etas_std::StdCollectionKind,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        use etas_std::StdCollectionKind as K;
        let comparator = if matches!(collection, K::PriorityQueue | K::OrderedMap | K::OrderedSet) {
            match args {
                [] => None,
                [InterpValue::Callable(target)] => {
                    if let Err(message) = self.validate_collection_comparator(target) {
                        return ControlSignal::invalid_arguments(message, span);
                    }
                    Some(target.clone())
                }
                _ => {
                    return ControlSignal::invalid_arguments(
                        format!(
                            "{}.new expects zero arguments or one pure comparison flow",
                            collection.name()
                        ),
                        span,
                    );
                }
            }
        } else {
            if !args.is_empty() {
                return ControlSignal::missing_checked_fact(
                    "checked collection constructor has unexpected arguments",
                    span,
                );
            }
            None
        };
        let value = match collection {
            K::Deque => InterpValue::Deque(crate::value::DequeValue::new(Vec::new())),
            K::Queue => InterpValue::Queue(crate::value::DequeValue::new(Vec::new())),
            K::Stack => InterpValue::Stack(ArrayValue::new(Vec::new())),
            K::PriorityQueue => {
                InterpValue::PriorityQueue(crate::value::PriorityQueueValue::new(comparator))
            }
            K::OrderedMap => {
                InterpValue::OrderedMap(crate::value::OrderedMapValue::new(comparator))
            }
            K::OrderedSet => {
                InterpValue::OrderedSet(crate::value::OrderedSetValue::new(comparator))
            }
            _ => {
                return ControlSignal::missing_checked_fact(
                    "unknown checked collection constructor",
                    span,
                );
            }
        };
        ControlSignal::Value(value)
    }
}

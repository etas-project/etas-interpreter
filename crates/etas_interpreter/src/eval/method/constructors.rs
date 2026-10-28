use super::*;

impl<'a> EvalContext<'a> {
    pub(in crate::eval) fn eval_range_type_method_values(
        &mut self,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        let bounds = match method {
            "closed" => crate::value::RangeBounds::ClosedClosed,
            "open" => crate::value::RangeBounds::OpenOpen,
            other => {
                return unsupported_method(span, "Range type", other);
            }
        };
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

    pub(in crate::eval) fn eval_advanced_collection_type_method(
        &mut self,
        collection: &str,
        method: &str,
        type_args: &[etas_hir::HirTypeId],
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        if method != "new" {
            return unsupported_method(span, &format!("{collection} type"), method);
        }
        if !args.is_empty() && !matches!(collection, "PriorityQueue" | "OrderedMap" | "OrderedSet")
        {
            return ControlSignal::invalid_arguments(
                format!("{collection}.new expects no method arguments"),
                span,
            );
        }
        let expected_type_args = match collection {
            "Deque" | "Queue" | "Stack" | "OrderedSet" => 1,
            "PriorityQueue" | "OrderedMap" => 2,
            _ => 0,
        };
        if type_args.len() != expected_type_args {
            return ControlSignal::missing_checked_fact(
                format!("{collection}.new requires checked type arguments"),
                span,
            );
        }
        let comparator = if matches!(collection, "PriorityQueue" | "OrderedMap" | "OrderedSet") {
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
                            "{collection}.new expects zero arguments or one pure comparison flow"
                        ),
                        span,
                    );
                }
            }
        } else {
            None
        };
        let value = match collection {
            "Deque" => InterpValue::Deque(crate::value::DequeValue::new(Vec::new())),
            "Queue" => InterpValue::Queue(crate::value::DequeValue::new(Vec::new())),
            "Stack" => InterpValue::Stack(ArrayValue::new(Vec::new())),
            "PriorityQueue" => {
                InterpValue::PriorityQueue(crate::value::PriorityQueueValue::new(comparator))
            }
            "OrderedMap" => InterpValue::OrderedMap(crate::value::OrderedMapValue::new(comparator)),
            "OrderedSet" => InterpValue::OrderedSet(crate::value::OrderedSetValue::new(comparator)),
            _ => {
                return ControlSignal::missing_checked_fact(
                    format!("unknown checked collection constructor `{collection}`"),
                    span,
                );
            }
        };
        ControlSignal::Value(value)
    }
}

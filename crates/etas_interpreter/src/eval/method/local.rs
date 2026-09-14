use super::*;

enum EvaluatedLocalArgs {
    None,
    One(InterpValue),
    Two(InterpValue, InterpValue),
}

impl EvaluatedLocalArgs {
    fn from_values(values: Vec<InterpValue>) -> Result<Self, ()> {
        let mut values = values.into_iter();
        match (values.next(), values.next(), values.next()) {
            (None, None, None) => Ok(Self::None),
            (Some(first), None, None) => Ok(Self::One(first)),
            (Some(first), Some(second), None) => Ok(Self::Two(first, second)),
            _ => Err(()),
        }
    }
}

impl<'a> EvalContext<'a> {
    pub(in crate::eval) fn eval_local_method_with_values(
        &mut self,
        expr: HirExprId,
        receiver: InterpValue,
        method: &str,
        type_args: &[etas_hir::HirTypeId],
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let Some(expected) = local_value_method_expected_arg_count(&receiver, method) else {
            return unsupported_method(span, local_receiver_name(&receiver), method);
        };
        if args.len() != expected {
            return ControlSignal::invalid_arguments(
                format!(
                    "{} expects exactly {expected} argument(s)",
                    local_method_label(&receiver, method)
                ),
                span,
            );
        }
        let Ok(args) = EvaluatedLocalArgs::from_values(args) else {
            return ControlSignal::missing_checked_fact(
                "collection method has no runtime argument layout",
                span,
            );
        };
        match receiver {
            InterpValue::Message(message) if method == "cast" => {
                if !matches!(args, EvaluatedLocalArgs::None) {
                    return ControlSignal::invalid_arguments(
                        "Message.cast expects no value arguments",
                        span,
                    );
                }
                if type_args.len() != 1 {
                    return ControlSignal::missing_checked_fact(
                        "Message.cast requires one checked target type argument",
                        span,
                    );
                }
                self.eval_checked_message_cast(message, type_args, span)
            }
            InterpValue::Array(values) => {
                self.eval_array_method_values(expr, values, method, args, span)
            }
            InterpValue::List(values) => self.eval_list_method_values(values, method, args, span),
            InterpValue::Slice(values) => {
                self.eval_slice_method_values(expr, values, method, args, span)
            }
            InterpValue::Map(entries) => self.eval_map_method_values(entries, method, args, span),
            InterpValue::Deque(values) => self.eval_deque_method_values(values, method, args, span),
            InterpValue::Queue(values) => self.eval_queue_method_values(values, method, args, span),
            InterpValue::Stack(values) => self.eval_stack_method_values(values, method, args, span),
            InterpValue::PriorityQueue(entries) => {
                self.eval_priority_queue_method_values(entries, method, args, span)
            }
            InterpValue::OrderedMap(entries) => {
                self.eval_ordered_map_method_values(entries, method, args, span)
            }
            InterpValue::OrderedSet(values) => {
                self.eval_ordered_set_method_values(values, method, args, span)
            }
            other => unsupported_method(span, local_receiver_name(&other), method),
        }
    }

    fn eval_array_method_values(
        &mut self,
        expr: HirExprId,
        values: ArrayValue,
        method: &str,
        args: EvaluatedLocalArgs,
        span: Span,
    ) -> ControlSignal {
        match (method, args) {
            ("len", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::usize(values.borrow().len()))
            }
            ("is_empty", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty()))
            }
            ("get", EvaluatedLocalArgs::One(arg)) => {
                let Some(index) = self.index_usize(arg, span) else {
                    return ControlSignal::Value(InterpValue::OptionNone);
                };
                ControlSignal::Value(
                    values
                        .borrow()
                        .get(index)
                        .cloned()
                        .map(crate::value::SharedValue::new)
                        .map(InterpValue::OptionSome)
                        .unwrap_or(InterpValue::OptionNone),
                )
            }
            ("at", EvaluatedLocalArgs::One(arg)) => {
                self.eval_index_value(expr, InterpValue::Array(values), arg, span)
            }
            ("push", EvaluatedLocalArgs::One(arg)) => {
                let mut next = values.into_values();
                next.push(arg);
                ControlSignal::Value(InterpValue::Array(ArrayValue::new(next)))
            }
            ("pop", EvaluatedLocalArgs::None) => {
                let mut next = values.into_values();
                let popped = next.pop();
                ControlSignal::Value(collection_pop_result(
                    InterpValue::Array(ArrayValue::new(next)),
                    popped,
                ))
            }
            ("extend", EvaluatedLocalArgs::One(arg)) => {
                let InterpValue::Array(other) = arg else {
                    return ControlSignal::invalid_arguments(
                        "Array.extend expects an Array value",
                        span,
                    );
                };
                ControlSignal::Value(InterpValue::Array(values.concat(other)))
            }
            _ => unsupported_collection_method(span, "Array", method),
        }
    }

    fn eval_list_method_values(
        &mut self,
        mut values: crate::value::ListValue,
        method: &str,
        args: EvaluatedLocalArgs,
        span: Span,
    ) -> ControlSignal {
        match (method, args) {
            ("len", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::usize(values.len()))
            }
            ("is_empty", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::Bool(values.is_empty()))
            }
            ("push", EvaluatedLocalArgs::One(arg)) => {
                values.push_front(arg);
                ControlSignal::Value(InterpValue::List(values))
            }
            ("pop", EvaluatedLocalArgs::None) => {
                let popped = values.pop_front();
                ControlSignal::Value(collection_pop_result(InterpValue::List(values), popped))
            }
            _ => unsupported_collection_method(span, "List", method),
        }
    }

    fn eval_slice_method_values(
        &mut self,
        expr: HirExprId,
        values: SliceValue,
        method: &str,
        args: EvaluatedLocalArgs,
        span: Span,
    ) -> ControlSignal {
        match (method, args) {
            ("len", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::usize(values.borrow().len()))
            }
            ("is_empty", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty()))
            }
            ("get", EvaluatedLocalArgs::One(arg)) => {
                let Some(index) = self.index_usize(arg, span) else {
                    return ControlSignal::Value(InterpValue::OptionNone);
                };
                ControlSignal::Value(
                    values
                        .borrow()
                        .get(index)
                        .cloned()
                        .map(crate::value::SharedValue::new)
                        .map(InterpValue::OptionSome)
                        .unwrap_or(InterpValue::OptionNone),
                )
            }
            ("at", EvaluatedLocalArgs::One(arg)) => {
                self.eval_index_value(expr, InterpValue::Slice(values), arg, span)
            }
            ("to_array", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::Array(ArrayValue::new(values.into_values())))
            }
            _ => unsupported_collection_method(span, "Slice", method),
        }
    }

    fn eval_map_method_values(
        &mut self,
        entries: MapValue,
        method: &str,
        args: EvaluatedLocalArgs,
        span: Span,
    ) -> ControlSignal {
        match (method, args) {
            ("len", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::usize(entries.borrow().len()))
            }
            ("is_empty", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::Bool(entries.borrow().is_empty()))
            }
            ("contains_key", EvaluatedLocalArgs::One(arg)) => {
                ControlSignal::Value(InterpValue::Bool(entries.contains_key(&arg)))
            }
            ("get", EvaluatedLocalArgs::One(arg)) => ControlSignal::Value(
                entries
                    .get(&arg)
                    .map(crate::value::SharedValue::new)
                    .map(InterpValue::OptionSome)
                    .unwrap_or(InterpValue::OptionNone),
            ),
            _ => unsupported_collection_method(span, "Map", method),
        }
    }

    fn eval_deque_method_values(
        &mut self,
        mut values: crate::value::DequeValue,
        method: &str,
        args: EvaluatedLocalArgs,
        span: Span,
    ) -> ControlSignal {
        match (method, args) {
            ("len", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::usize(values.borrow().len()))
            }
            ("is_empty", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty()))
            }
            ("push_front" | "push_back", EvaluatedLocalArgs::One(arg)) => {
                if method == "push_front" {
                    values.push_front(arg);
                } else {
                    values.push_back(arg);
                }
                ControlSignal::Value(InterpValue::Deque(values))
            }
            ("pop_front" | "pop_back", EvaluatedLocalArgs::None) => {
                let popped = if method == "pop_front" {
                    values.pop_front()
                } else {
                    values.pop_back()
                };
                ControlSignal::Value(collection_pop_result(InterpValue::Deque(values), popped))
            }
            _ => unsupported_collection_method(span, "Deque", method),
        }
    }

    fn eval_queue_method_values(
        &mut self,
        mut values: crate::value::DequeValue,
        method: &str,
        args: EvaluatedLocalArgs,
        span: Span,
    ) -> ControlSignal {
        match (method, args) {
            ("len", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::usize(values.borrow().len()))
            }
            ("is_empty", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty()))
            }
            ("push", EvaluatedLocalArgs::One(arg)) => {
                values.push_back(arg);
                ControlSignal::Value(InterpValue::Queue(values))
            }
            ("pop", EvaluatedLocalArgs::None) => {
                let popped = values.pop_front();
                ControlSignal::Value(collection_pop_result(InterpValue::Queue(values), popped))
            }
            _ => unsupported_collection_method(span, "Queue", method),
        }
    }

    fn eval_stack_method_values(
        &mut self,
        values: ArrayValue,
        method: &str,
        args: EvaluatedLocalArgs,
        span: Span,
    ) -> ControlSignal {
        match (method, args) {
            ("len", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::usize(values.borrow().len()))
            }
            ("is_empty", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty()))
            }
            ("push", EvaluatedLocalArgs::One(arg)) => {
                let mut next = values.into_values();
                next.push(arg);
                ControlSignal::Value(InterpValue::Stack(ArrayValue::new(next)))
            }
            ("pop", EvaluatedLocalArgs::None) => {
                let mut next = values.into_values();
                let popped = next.pop();
                ControlSignal::Value(collection_pop_result(
                    InterpValue::Stack(ArrayValue::new(next)),
                    popped,
                ))
            }
            _ => unsupported_collection_method(span, "Stack", method),
        }
    }

    fn eval_priority_queue_method_values(
        &mut self,
        entries: MapValue,
        method: &str,
        args: EvaluatedLocalArgs,
        span: Span,
    ) -> ControlSignal {
        match (method, args) {
            ("len", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::usize(entries.borrow().len()))
            }
            ("is_empty", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::Bool(entries.borrow().is_empty()))
            }
            ("push", EvaluatedLocalArgs::Two(value, priority)) => {
                let mut next = entries.into_values();
                next.push((priority, value));
                ControlSignal::Value(InterpValue::PriorityQueue(MapValue::new(next)))
            }
            ("pop", EvaluatedLocalArgs::None) => {
                let mut next = entries.into_values();
                let popped = next.pop().map(|(_, value)| value);
                ControlSignal::Value(collection_pop_result(
                    InterpValue::PriorityQueue(MapValue::new(next)),
                    popped,
                ))
            }
            _ => unsupported_collection_method(span, "PriorityQueue", method),
        }
    }

    fn eval_ordered_map_method_values(
        &mut self,
        mut entries: MapValue,
        method: &str,
        args: EvaluatedLocalArgs,
        span: Span,
    ) -> ControlSignal {
        match (method, args) {
            ("len", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::usize(entries.borrow().len()))
            }
            ("is_empty", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::Bool(entries.borrow().is_empty()))
            }
            ("contains_key", EvaluatedLocalArgs::One(arg)) => {
                ControlSignal::Value(InterpValue::Bool(entries.contains_key(&arg)))
            }
            ("get", EvaluatedLocalArgs::One(arg)) => ControlSignal::Value(
                entries
                    .get(&arg)
                    .map(crate::value::SharedValue::new)
                    .map(InterpValue::OptionSome)
                    .unwrap_or(InterpValue::OptionNone),
            ),
            ("insert", EvaluatedLocalArgs::Two(key, value)) => {
                entries.insert(key, value);
                ControlSignal::Value(InterpValue::OrderedMap(entries))
            }
            _ => unsupported_collection_method(span, "OrderedMap", method),
        }
    }

    fn eval_ordered_set_method_values(
        &mut self,
        values: crate::value::SetValue,
        method: &str,
        args: EvaluatedLocalArgs,
        span: Span,
    ) -> ControlSignal {
        match (method, args) {
            ("len", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::usize(values.borrow().len()))
            }
            ("is_empty", EvaluatedLocalArgs::None) => {
                ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty()))
            }
            ("contains", EvaluatedLocalArgs::One(arg)) => {
                ControlSignal::Value(InterpValue::Bool(values.contains(&arg)))
            }
            ("insert", EvaluatedLocalArgs::One(arg)) => {
                let mut next = values;
                next.insert(arg);
                ControlSignal::Value(InterpValue::OrderedSet(next))
            }
            _ => unsupported_collection_method(span, "OrderedSet", method),
        }
    }
}

#[cfg(test)]
#[path = "local_tests.rs"]
mod tests;

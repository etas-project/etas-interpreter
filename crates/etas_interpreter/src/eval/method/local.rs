use super::*;

impl<'a> EvalContext<'a> {
    pub(in crate::eval) fn eval_local_method_with_values(
        &mut self,
        expr: HirExprId,
        receiver: InterpValue,
        method: &str,
        type_args: &[etas_hir::HirTypeId],
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match receiver {
            InterpValue::Message(message) if method == "cast" => {
                if !args.is_empty() {
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

    pub(in crate::eval) fn eval_array_method_values(
        &mut self,
        expr: HirExprId,
        values: ArrayValue,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match method {
            "len" => ControlSignal::Value(InterpValue::usize(values.borrow().len())),
            "is_empty" => ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty())),
            "get" => {
                let Some(index) = self.index_usize(args[0].clone(), span) else {
                    return ControlSignal::Value(InterpValue::OptionNone);
                };
                ControlSignal::Value(
                    values
                        .borrow()
                        .get(index)
                        .cloned()
                        .map(Box::new)
                        .map(InterpValue::OptionSome)
                        .unwrap_or(InterpValue::OptionNone),
                )
            }
            "at" => self.eval_index_value(expr, InterpValue::Array(values), args[0].clone(), span),
            "push" => {
                let mut next = values.snapshot();
                next.push(args[0].clone());
                ControlSignal::Value(InterpValue::Array(ArrayValue::new(next)))
            }
            "pop" => {
                let mut next = values.snapshot();
                let popped = next.pop();
                ControlSignal::Value(collection_pop_result(
                    InterpValue::Array(ArrayValue::new(next)),
                    popped,
                ))
            }
            "extend" => {
                let InterpValue::Array(other) = args[0].clone() else {
                    return ControlSignal::invalid_arguments(
                        "Array.extend expects an Array value",
                        span,
                    );
                };
                let mut next = values.snapshot();
                next.extend(other.snapshot());
                ControlSignal::Value(InterpValue::Array(ArrayValue::new(next)))
            }
            _ => unsupported_collection_method(span, "Array", method),
        }
    }

    pub(in crate::eval) fn eval_list_method_values(
        &mut self,
        values: crate::value::ListValue,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match method {
            "len" => ControlSignal::Value(InterpValue::usize(values.borrow().len())),
            "is_empty" => ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty())),
            "push" => {
                let mut next = values.snapshot();
                next.insert(0, args[0].clone());
                ControlSignal::Value(InterpValue::List(next.into()))
            }
            "pop" => {
                let mut next = values.snapshot();
                let popped = if next.is_empty() {
                    None
                } else {
                    Some(next.remove(0))
                };
                ControlSignal::Value(collection_pop_result(
                    InterpValue::List(next.into()),
                    popped,
                ))
            }
            _ => unsupported_collection_method(span, "List", method),
        }
    }

    pub(in crate::eval) fn eval_slice_method_values(
        &mut self,
        expr: HirExprId,
        values: SliceValue,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match method {
            "len" => ControlSignal::Value(InterpValue::usize(values.borrow().len())),
            "is_empty" => ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty())),
            "get" => {
                let Some(index) = self.index_usize(args[0].clone(), span) else {
                    return ControlSignal::Value(InterpValue::OptionNone);
                };
                ControlSignal::Value(
                    values
                        .borrow()
                        .get(index)
                        .cloned()
                        .map(Box::new)
                        .map(InterpValue::OptionSome)
                        .unwrap_or(InterpValue::OptionNone),
                )
            }
            "at" => self.eval_index_value(expr, InterpValue::Slice(values), args[0].clone(), span),
            "to_array" => {
                ControlSignal::Value(InterpValue::Array(ArrayValue::new(values.snapshot())))
            }
            _ => unsupported_collection_method(span, "Slice", method),
        }
    }

    pub(in crate::eval) fn eval_map_method_values(
        &mut self,
        entries: MapValue,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match method {
            "len" => ControlSignal::Value(InterpValue::usize(entries.borrow().len())),
            "is_empty" => ControlSignal::Value(InterpValue::Bool(entries.borrow().is_empty())),
            "contains_key" => ControlSignal::Value(InterpValue::Bool(
                entries
                    .snapshot()
                    .iter()
                    .any(|(candidate, _)| candidate == &args[0]),
            )),
            "get" => ControlSignal::Value(
                entries
                    .snapshot()
                    .into_iter()
                    .find_map(|(candidate, value)| (candidate == args[0]).then_some(value))
                    .map(Box::new)
                    .map(InterpValue::OptionSome)
                    .unwrap_or(InterpValue::OptionNone),
            ),
            _ => unsupported_collection_method(span, "Map", method),
        }
    }

    pub(in crate::eval) fn eval_deque_method_values(
        &mut self,
        values: ArrayValue,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match method {
            "len" => ControlSignal::Value(InterpValue::usize(values.borrow().len())),
            "is_empty" => ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty())),
            "push_front" | "push_back" => {
                let mut next = values.snapshot();
                if method == "push_front" {
                    next.insert(0, args[0].clone());
                } else {
                    next.push(args[0].clone());
                }
                ControlSignal::Value(InterpValue::Deque(ArrayValue::new(next)))
            }
            "pop_front" | "pop_back" => {
                let mut next = values.snapshot();
                let popped = if method == "pop_front" {
                    (!next.is_empty()).then(|| next.remove(0))
                } else {
                    next.pop()
                };
                ControlSignal::Value(collection_pop_result(
                    InterpValue::Deque(ArrayValue::new(next)),
                    popped,
                ))
            }
            _ => unsupported_collection_method(span, "Deque", method),
        }
    }

    pub(in crate::eval) fn eval_queue_method_values(
        &mut self,
        values: ArrayValue,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match method {
            "len" => ControlSignal::Value(InterpValue::usize(values.borrow().len())),
            "is_empty" => ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty())),
            "push" => {
                let mut next = values.snapshot();
                next.push(args[0].clone());
                ControlSignal::Value(InterpValue::Queue(ArrayValue::new(next)))
            }
            "pop" => {
                let mut next = values.snapshot();
                let popped = (!next.is_empty()).then(|| next.remove(0));
                ControlSignal::Value(collection_pop_result(
                    InterpValue::Queue(ArrayValue::new(next)),
                    popped,
                ))
            }
            _ => unsupported_collection_method(span, "Queue", method),
        }
    }

    pub(in crate::eval) fn eval_stack_method_values(
        &mut self,
        values: ArrayValue,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match method {
            "len" => ControlSignal::Value(InterpValue::usize(values.borrow().len())),
            "is_empty" => ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty())),
            "push" => {
                let mut next = values.snapshot();
                next.push(args[0].clone());
                ControlSignal::Value(InterpValue::Stack(ArrayValue::new(next)))
            }
            "pop" => {
                let mut next = values.snapshot();
                let popped = next.pop();
                ControlSignal::Value(collection_pop_result(
                    InterpValue::Stack(ArrayValue::new(next)),
                    popped,
                ))
            }
            _ => unsupported_collection_method(span, "Stack", method),
        }
    }

    pub(in crate::eval) fn eval_priority_queue_method_values(
        &mut self,
        entries: MapValue,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match method {
            "len" => ControlSignal::Value(InterpValue::usize(entries.borrow().len())),
            "is_empty" => ControlSignal::Value(InterpValue::Bool(entries.borrow().is_empty())),
            "push" => {
                let mut next = entries.snapshot();
                next.push((args[1].clone(), args[0].clone()));
                ControlSignal::Value(InterpValue::PriorityQueue(MapValue::new(next)))
            }
            "pop" => {
                let mut next = entries.snapshot();
                let popped = next.pop().map(|(_, value)| value);
                ControlSignal::Value(collection_pop_result(
                    InterpValue::PriorityQueue(MapValue::new(next)),
                    popped,
                ))
            }
            _ => unsupported_collection_method(span, "PriorityQueue", method),
        }
    }

    pub(in crate::eval) fn eval_ordered_map_method_values(
        &mut self,
        entries: MapValue,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match method {
            "len" => ControlSignal::Value(InterpValue::usize(entries.borrow().len())),
            "is_empty" => ControlSignal::Value(InterpValue::Bool(entries.borrow().is_empty())),
            "contains_key" => ControlSignal::Value(InterpValue::Bool(
                entries
                    .snapshot()
                    .iter()
                    .any(|(candidate, _)| candidate == &args[0]),
            )),
            "get" => ControlSignal::Value(
                entries
                    .snapshot()
                    .into_iter()
                    .find_map(|(candidate, value)| (candidate == args[0]).then_some(value))
                    .map(Box::new)
                    .map(InterpValue::OptionSome)
                    .unwrap_or(InterpValue::OptionNone),
            ),
            "insert" => {
                let key = args[0].clone();
                let value = args[1].clone();
                let mut next = entries.snapshot();
                if let Some((_, existing)) =
                    next.iter_mut().find(|(candidate, _)| candidate == &key)
                {
                    *existing = value;
                } else {
                    next.push((key, value));
                }
                ControlSignal::Value(InterpValue::OrderedMap(MapValue::new(next)))
            }
            _ => unsupported_collection_method(span, "OrderedMap", method),
        }
    }

    pub(in crate::eval) fn eval_ordered_set_method_values(
        &mut self,
        values: crate::value::SetValue,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        match method {
            "len" => ControlSignal::Value(InterpValue::usize(values.borrow().len())),
            "is_empty" => ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty())),
            "contains" => ControlSignal::Value(InterpValue::Bool(
                values
                    .snapshot()
                    .iter()
                    .any(|candidate| candidate == &args[0]),
            )),
            "insert" => {
                let mut next = values.snapshot();
                if !next.iter().any(|candidate| candidate == &args[0]) {
                    next.push(args[0].clone());
                }
                ControlSignal::Value(InterpValue::OrderedSet(next.into()))
            }
            _ => unsupported_collection_method(span, "OrderedSet", method),
        }
    }
}

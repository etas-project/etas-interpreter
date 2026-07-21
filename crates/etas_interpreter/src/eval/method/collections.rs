use super::*;

impl<'a> EvalContext<'a> {
    pub(in crate::eval) fn eval_array_method(
        &mut self,
        expr: HirExprId,
        values: ArrayValue,
        method: &str,
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match method {
            "len" => {
                if let Some(fault) = self.expect_no_args(args, span, "Array.len") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::usize(values.borrow().len()))
            }
            "is_empty" => {
                if let Some(fault) = self.expect_no_args(args, span, "Array.is_empty") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty()))
            }
            "get" => {
                let index = eval_method_arg!(self.eval_one_arg(args, span, "Array.get", frame));
                let Some(index) = self.index_usize(index, span) else {
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
            "at" => {
                let index = eval_method_arg!(self.eval_one_arg(args, span, "Array.at", frame));
                self.eval_index_value(expr, InterpValue::Array(values), index, span)
            }
            "push" => {
                let value = eval_method_arg!(self.eval_one_arg(args, span, "Array.push", frame));
                let mut next = values.snapshot();
                next.push(value);
                ControlSignal::Value(InterpValue::Array(ArrayValue::new(next)))
            }
            "pop" => {
                if let Some(fault) = self.expect_no_args(args, span, "Array.pop") {
                    return fault;
                }
                let mut next = values.snapshot();
                let popped = next.pop();
                ControlSignal::Value(collection_pop_result(
                    InterpValue::Array(ArrayValue::new(next)),
                    popped,
                ))
            }
            "extend" => {
                let value = eval_method_arg!(self.eval_one_arg(args, span, "Array.extend", frame));
                let InterpValue::Array(other) = value else {
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

    pub(in crate::eval) fn eval_list_method(
        &mut self,
        values: crate::value::ListValue,
        method: &str,
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match method {
            "len" => {
                if let Some(fault) = self.expect_no_args(args, span, "List.len") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::usize(values.borrow().len()))
            }
            "is_empty" => {
                if let Some(fault) = self.expect_no_args(args, span, "List.is_empty") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty()))
            }
            "push" => {
                let value = eval_method_arg!(self.eval_one_arg(args, span, "List.push", frame));
                let mut next = values.snapshot();
                next.insert(0, value);
                ControlSignal::Value(InterpValue::List(next.into()))
            }
            "pop" => {
                if let Some(fault) = self.expect_no_args(args, span, "List.pop") {
                    return fault;
                }
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

    pub(in crate::eval) fn eval_slice_method(
        &mut self,
        expr: HirExprId,
        values: SliceValue,
        method: &str,
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match method {
            "len" => {
                if let Some(fault) = self.expect_no_args(args, span, "Slice.len") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::usize(values.borrow().len()))
            }
            "is_empty" => {
                if let Some(fault) = self.expect_no_args(args, span, "Slice.is_empty") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty()))
            }
            "get" => {
                let index = eval_method_arg!(self.eval_one_arg(args, span, "Slice.get", frame));
                let Some(index) = self.index_usize(index, span) else {
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
            "at" => {
                let index = eval_method_arg!(self.eval_one_arg(args, span, "Slice.at", frame));
                self.eval_index_value(expr, InterpValue::Slice(values), index, span)
            }
            "to_array" => {
                if let Some(fault) = self.expect_no_args(args, span, "Slice.to_array") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::Array(ArrayValue::new(values.snapshot())))
            }
            _ => unsupported_collection_method(span, "Slice", method),
        }
    }

    pub(in crate::eval) fn eval_map_method(
        &mut self,
        entries: MapValue,
        method: &str,
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match method {
            "len" => {
                if let Some(fault) = self.expect_no_args(args, span, "Map.len") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::usize(entries.borrow().len()))
            }
            "is_empty" => {
                if let Some(fault) = self.expect_no_args(args, span, "Map.is_empty") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::Bool(entries.borrow().is_empty()))
            }
            "contains_key" => {
                let key =
                    eval_method_arg!(self.eval_one_arg(args, span, "Map.contains_key", frame));
                ControlSignal::Value(InterpValue::Bool(
                    entries
                        .snapshot()
                        .iter()
                        .any(|(candidate, _)| candidate == &key),
                ))
            }
            "get" => {
                let key = eval_method_arg!(self.eval_one_arg(args, span, "Map.get", frame));
                ControlSignal::Value(
                    entries
                        .snapshot()
                        .into_iter()
                        .find_map(|(candidate, value)| (candidate == key).then_some(value))
                        .map(Box::new)
                        .map(InterpValue::OptionSome)
                        .unwrap_or(InterpValue::OptionNone),
                )
            }
            _ => unsupported_collection_method(span, "Map", method),
        }
    }

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
        args: &[HirArg],
        span: Span,
    ) -> ControlSignal {
        if method != "new" {
            return unsupported_method(span, &format!("{collection} type"), method);
        }
        if !args.is_empty() {
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
        let value = match collection {
            "Deque" => InterpValue::Deque(ArrayValue::new(Vec::new())),
            "Queue" => InterpValue::Queue(ArrayValue::new(Vec::new())),
            "Stack" => InterpValue::Stack(ArrayValue::new(Vec::new())),
            "PriorityQueue" => InterpValue::PriorityQueue(MapValue::new(Vec::new())),
            "OrderedMap" => InterpValue::OrderedMap(MapValue::new(Vec::new())),
            "OrderedSet" => InterpValue::OrderedSet(Vec::new().into()),
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

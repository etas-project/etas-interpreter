use super::*;

impl<'a> EvalContext<'a> {
    pub(in crate::eval) fn eval_deque_method(
        &mut self,
        values: ArrayValue,
        method: &str,
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match method {
            "len" => {
                if let Some(fault) = self.expect_no_args(args, span, "Deque.len") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::usize(values.borrow().len()))
            }
            "is_empty" => {
                if let Some(fault) = self.expect_no_args(args, span, "Deque.is_empty") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty()))
            }
            "push_front" | "push_back" => {
                let value = eval_method_arg!(self.eval_one_arg(args, span, method, frame));
                let mut next = values.snapshot();
                if method == "push_front" {
                    next.insert(0, value);
                } else {
                    next.push(value);
                }
                ControlSignal::Value(InterpValue::Deque(ArrayValue::new(next)))
            }
            "pop_front" | "pop_back" => {
                if let Some(fault) = self.expect_no_args(args, span, method) {
                    return fault;
                }
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

    pub(in crate::eval) fn eval_queue_method(
        &mut self,
        values: ArrayValue,
        method: &str,
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match method {
            "len" => {
                if let Some(fault) = self.expect_no_args(args, span, "Queue.len") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::usize(values.borrow().len()))
            }
            "is_empty" => {
                if let Some(fault) = self.expect_no_args(args, span, "Queue.is_empty") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty()))
            }
            "push" => {
                let value = eval_method_arg!(self.eval_one_arg(args, span, "Queue.push", frame));
                let mut next = values.snapshot();
                next.push(value);
                ControlSignal::Value(InterpValue::Queue(ArrayValue::new(next)))
            }
            "pop" => {
                if let Some(fault) = self.expect_no_args(args, span, "Queue.pop") {
                    return fault;
                }
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

    pub(in crate::eval) fn eval_stack_method(
        &mut self,
        values: ArrayValue,
        method: &str,
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match method {
            "len" => {
                if let Some(fault) = self.expect_no_args(args, span, "Stack.len") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::usize(values.borrow().len()))
            }
            "is_empty" => {
                if let Some(fault) = self.expect_no_args(args, span, "Stack.is_empty") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty()))
            }
            "push" => {
                let value = eval_method_arg!(self.eval_one_arg(args, span, "Stack.push", frame));
                let mut next = values.snapshot();
                next.push(value);
                ControlSignal::Value(InterpValue::Stack(ArrayValue::new(next)))
            }
            "pop" => {
                if let Some(fault) = self.expect_no_args(args, span, "Stack.pop") {
                    return fault;
                }
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

    pub(in crate::eval) fn eval_priority_queue_method(
        &mut self,
        entries: MapValue,
        method: &str,
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match method {
            "len" => {
                if let Some(fault) = self.expect_no_args(args, span, "PriorityQueue.len") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::usize(entries.borrow().len()))
            }
            "is_empty" => {
                if let Some(fault) = self.expect_no_args(args, span, "PriorityQueue.is_empty") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::Bool(entries.borrow().is_empty()))
            }
            "push" => {
                let [value, priority] = eval_method_arg!(self.eval_exact_args(
                    args,
                    2,
                    span,
                    "PriorityQueue.push",
                    frame,
                ));
                let mut next = entries.snapshot();
                next.push((priority, value));
                ControlSignal::Value(InterpValue::PriorityQueue(MapValue::new(next)))
            }
            "pop" => {
                if let Some(fault) = self.expect_no_args(args, span, "PriorityQueue.pop") {
                    return fault;
                }
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

    pub(in crate::eval) fn eval_ordered_map_method(
        &mut self,
        entries: MapValue,
        method: &str,
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match method {
            "len" => {
                if let Some(fault) = self.expect_no_args(args, span, "OrderedMap.len") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::usize(entries.borrow().len()))
            }
            "is_empty" => {
                if let Some(fault) = self.expect_no_args(args, span, "OrderedMap.is_empty") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::Bool(entries.borrow().is_empty()))
            }
            "contains_key" => {
                let key = eval_method_arg!(self.eval_one_arg(
                    args,
                    span,
                    "OrderedMap.contains_key",
                    frame,
                ));
                ControlSignal::Value(InterpValue::Bool(
                    entries
                        .snapshot()
                        .iter()
                        .any(|(candidate, _)| candidate == &key),
                ))
            }
            "get" => {
                let key = eval_method_arg!(self.eval_one_arg(args, span, "OrderedMap.get", frame));
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
            "insert" => {
                let [key, value] = eval_method_arg!(self.eval_exact_args(
                    args,
                    2,
                    span,
                    "OrderedMap.insert",
                    frame,
                ));
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

    pub(in crate::eval) fn eval_ordered_set_method(
        &mut self,
        values: crate::value::SetValue,
        method: &str,
        args: &[HirArg],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match method {
            "len" => {
                if let Some(fault) = self.expect_no_args(args, span, "OrderedSet.len") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::usize(values.borrow().len()))
            }
            "is_empty" => {
                if let Some(fault) = self.expect_no_args(args, span, "OrderedSet.is_empty") {
                    return fault;
                }
                ControlSignal::Value(InterpValue::Bool(values.borrow().is_empty()))
            }
            "contains" => {
                let value =
                    eval_method_arg!(self.eval_one_arg(args, span, "OrderedSet.contains", frame,));
                ControlSignal::Value(InterpValue::Bool(
                    values
                        .snapshot()
                        .iter()
                        .any(|candidate| candidate == &value),
                ))
            }
            "insert" => {
                let value =
                    eval_method_arg!(self.eval_one_arg(args, span, "OrderedSet.insert", frame,));
                let mut next = values.snapshot();
                if !next.iter().any(|candidate| candidate == &value) {
                    next.push(value);
                }
                ControlSignal::Value(InterpValue::OrderedSet(next.into()))
            }
            _ => unsupported_collection_method(span, "OrderedSet", method),
        }
    }

    pub(in crate::eval) fn resume_local_method_args(
        &mut self,
        state: LocalMethodArgsState,
        frame: &mut Frame,
    ) -> ControlSignal {
        let LocalMethodArgsState {
            expr,
            receiver,
            method,
            type_args,
            args,
            start_arg_index,
            mut evaluated_args,
            span,
        } = state;
        let Some(expected) = local_value_method_expected_arg_count(&receiver, &method) else {
            return self.eval_method_on_receiver(
                MethodDispatch {
                    expr,
                    method: &method,
                    type_args: &type_args,
                    args: &args,
                    span,
                },
                receiver,
                frame,
            );
        };
        if args.len() != expected {
            return ControlSignal::invalid_arguments(
                format!(
                    "{} expects exactly {} argument(s)",
                    local_method_label(&receiver, &method),
                    expected
                ),
                span,
            );
        }
        for (index, arg) in args.iter().enumerate().skip(start_arg_index) {
            let arg_expr = match arg {
                HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
            };
            match self.eval_expr(arg_expr, frame) {
                ControlSignal::Value(value) => evaluated_args.push(value),
                ControlSignal::Apply(pending) => {
                    return attach_local_method_args_continuation(
                        ControlSignal::Apply(pending),
                        LocalMethodArgContinuation {
                            expr,
                            receiver,
                            method: &method,
                            type_args: &type_args,
                            args,
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame,
                        },
                    );
                }
                ControlSignal::Checkpoint(pending) => {
                    return ControlSignal::Checkpoint(pending);
                }
                ControlSignal::Block(pending) => {
                    return attach_local_method_args_continuation(
                        ControlSignal::Block(pending),
                        LocalMethodArgContinuation {
                            expr,
                            receiver,
                            method: &method,
                            type_args: &type_args,
                            args,
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame,
                        },
                    );
                }
                ControlSignal::Expr(pending) => {
                    return attach_local_method_args_continuation(
                        ControlSignal::Expr(pending),
                        LocalMethodArgContinuation {
                            expr,
                            receiver,
                            method: &method,
                            type_args: &type_args,
                            args,
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame,
                        },
                    );
                }
                ControlSignal::Call(call) => {
                    return attach_local_method_args_continuation(
                        ControlSignal::Call(call),
                        LocalMethodArgContinuation {
                            expr,
                            receiver,
                            method: &method,
                            type_args: &type_args,
                            args,
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame,
                        },
                    );
                }
                ControlSignal::Memory(memory) => {
                    return attach_local_method_args_continuation(
                        ControlSignal::Memory(memory),
                        LocalMethodArgContinuation {
                            expr,
                            receiver,
                            method: &method,
                            type_args: &type_args,
                            args,
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame,
                        },
                    );
                }
                ControlSignal::Session(session) => {
                    return attach_local_method_args_continuation(
                        ControlSignal::Session(session),
                        LocalMethodArgContinuation {
                            expr,
                            receiver,
                            method: &method,
                            type_args: &type_args,
                            args,
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame,
                        },
                    );
                }
                ControlSignal::Perform(perform) => {
                    return attach_local_method_args_continuation(
                        ControlSignal::Perform(perform),
                        LocalMethodArgContinuation {
                            expr,
                            receiver,
                            method: &method,
                            type_args: &type_args,
                            args,
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame,
                        },
                    );
                }
                ControlSignal::Console(console) => {
                    return attach_local_method_args_continuation(
                        ControlSignal::Console(console),
                        LocalMethodArgContinuation {
                            expr,
                            receiver,
                            method: &method,
                            type_args: &type_args,
                            args,
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame,
                        },
                    );
                }
                ControlSignal::Command(command) => {
                    return attach_local_method_args_continuation(
                        ControlSignal::Command(command),
                        LocalMethodArgContinuation {
                            expr,
                            receiver,
                            method: &method,
                            type_args: &type_args,
                            args,
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame,
                        },
                    );
                }
                ControlSignal::Model(model) => {
                    return attach_local_method_args_continuation(
                        ControlSignal::Model(model),
                        LocalMethodArgContinuation {
                            expr,
                            receiver,
                            method: &method,
                            type_args: &type_args,
                            args,
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame,
                        },
                    );
                }
                ControlSignal::Host(host) => {
                    return attach_local_method_args_continuation(
                        ControlSignal::Host(host),
                        LocalMethodArgContinuation {
                            expr,
                            receiver,
                            method: &method,
                            type_args: &type_args,
                            args,
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame,
                        },
                    );
                }
                ControlSignal::Return(value) => {
                    return ControlSignal::invalid_arguments(
                        format!(
                            "{} argument returned unexpectedly: {value:?}",
                            local_method_label(&receiver, &method)
                        ),
                        span,
                    );
                }
                ControlSignal::Resume(value) => return ControlSignal::Resume(value),
                ControlSignal::Finish(value) => return ControlSignal::Finish(value),
                ControlSignal::Break => return ControlSignal::Break,
                ControlSignal::Fault(fault) => return ControlSignal::Fault(fault),
                ControlSignal::Continue => return ControlSignal::Continue,
            }
        }
        self.eval_local_method_with_values(
            expr,
            receiver,
            &method,
            &type_args,
            &evaluated_args,
            span,
        )
    }
}

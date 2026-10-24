use super::*;

impl<'a> EvalContext<'a> {
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
            return unsupported_method(span, local_receiver_name(&receiver), &method);
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
                ControlSignal::Cancelled(cause) => return ControlSignal::Cancelled(cause),
                ControlSignal::Continue => return ControlSignal::Continue,
            }
        }
        self.eval_local_method_with_values(
            expr,
            receiver,
            &method,
            &type_args,
            evaluated_args,
            span,
        )
    }
}

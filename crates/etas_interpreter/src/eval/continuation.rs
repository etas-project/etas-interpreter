use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn resume_perform(
        &mut self,
        perform: PendingPerform,
        value: InterpValue,
    ) -> ControlSignal {
        self.apply_continuation(perform.continuation, value)
    }

    pub(crate) fn propagate_perform_signal(
        &mut self,
        mut perform: PendingPerform,
    ) -> ControlSignal {
        let continuation = std::mem::replace(&mut perform.continuation, Continuation::BlockValue);
        self.apply_perform_to_continuation(perform, continuation)
    }

    fn apply_perform_to_continuation(
        &mut self,
        mut perform: PendingPerform,
        continuation: Continuation,
    ) -> ControlSignal {
        perform.continuation = Continuation::BlockValue;
        let mut signal = ControlSignal::pending_perform(perform);
        let mut work = vec![continuation];

        while let Some(continuation) = work.pop() {
            match continuation {
                Continuation::Chain { inner, outer } => {
                    work.push(*outer);
                    work.push(*inner);
                }
                Continuation::HandleBoundary {
                    scope_id,
                    inner,
                    handlers,
                    span,
                    frame,
                } if !matches!(*inner, Continuation::BlockValue) => {
                    work.push(Continuation::HandleBoundary {
                        scope_id,
                        inner: Box::new(Continuation::BlockValue),
                        handlers,
                        span,
                        frame,
                    });
                    work.push(*inner);
                }
                continuation => {
                    signal = match signal {
                        ControlSignal::Perform(mut pending) => match continuation {
                            Continuation::TryExpr { expr, span } => {
                                if self.can_capture_error_raise(expr, &pending) {
                                    self.capture_error_raise(expr, &pending, span)
                                } else {
                                    pending.continuation = compose_continuation(
                                        pending.continuation,
                                        Continuation::TryExpr { expr, span },
                                    );
                                    ControlSignal::Perform(pending)
                                }
                            }
                            Continuation::HandleBoundary {
                                scope_id,
                                inner: _,
                                handlers,
                                span,
                                mut frame,
                            } => self.continue_handle_signal(
                                ControlSignal::Perform(pending),
                                scope_id,
                                handlers,
                                span,
                                &mut frame,
                            ),
                            continuation => {
                                pending.continuation =
                                    compose_continuation(pending.continuation, continuation);
                                ControlSignal::Perform(pending)
                            }
                        },
                        signal => self.continue_chain_signal(signal, continuation),
                    };
                }
            }
        }

        signal
    }

    pub(crate) fn continue_chain_signal(
        &mut self,
        signal: ControlSignal,
        outer: Continuation,
    ) -> ControlSignal {
        match signal {
            ControlSignal::Apply(mut pending) => {
                pending.continuation = compose_continuation(pending.continuation, outer);
                ControlSignal::Apply(pending)
            }
            ControlSignal::Value(value) => self.apply_continuation(outer, value),
            ControlSignal::Return(value) => self.propagate_return_to_continuation(value, outer),
            ControlSignal::Checkpoint(pending) => ControlSignal::Checkpoint(pending),
            ControlSignal::Block(mut pending) => {
                pending.continuation = compose_continuation(pending.continuation, outer);
                ControlSignal::Block(pending)
            }
            ControlSignal::Memory(mut pending) => {
                pending.continuation = compose_continuation(pending.continuation, outer);
                ControlSignal::Memory(pending)
            }
            ControlSignal::Session(mut pending) => {
                pending.continuation = compose_continuation(pending.continuation, outer);
                ControlSignal::Session(pending)
            }
            ControlSignal::Perform(pending) => {
                let mut pending = *pending;
                pending.continuation = compose_continuation(pending.continuation, outer);
                self.propagate_perform_signal(pending)
            }
            ControlSignal::Console(mut pending) => {
                pending.continuation = compose_continuation(pending.continuation, outer);
                ControlSignal::Console(pending)
            }
            ControlSignal::Command(mut pending) => {
                pending.continuation = compose_continuation(pending.continuation, outer);
                ControlSignal::Command(pending)
            }
            ControlSignal::Model(mut pending) => {
                pending.continuation = compose_continuation(pending.continuation, outer);
                ControlSignal::Model(pending)
            }
            ControlSignal::Host(mut pending) => {
                pending.continuation = compose_continuation(pending.continuation, outer);
                ControlSignal::Host(pending)
            }
            ControlSignal::Call(mut pending) => {
                pending.continuation = compose_continuation(pending.continuation, outer);
                ControlSignal::Call(pending)
            }
            ControlSignal::Expr(mut pending) => {
                pending.continuation = compose_continuation(pending.continuation, outer);
                ControlSignal::Expr(pending)
            }
            ControlSignal::Resume(value) => self.propagate_resume_to_continuation(value, outer),
            ControlSignal::Finish(value) => self.propagate_finish_to_continuation(value, outer),
            ControlSignal::Fault(fault) => ControlSignal::Fault(fault),
            ControlSignal::Cancelled(cause) => ControlSignal::Cancelled(cause),
            signal @ (ControlSignal::Break | ControlSignal::Continue) => {
                if let Continuation::RetryAttempt { retry, .. } = &outer {
                    self.finish_retry_attempt_success(retry);
                    signal
                } else {
                    self.propagate_loop_control_to_continuation(signal, outer)
                }
            }
        }
    }

    fn propagate_return_to_continuation(
        &mut self,
        value: InterpValue,
        continuation: Continuation,
    ) -> ControlSignal {
        self.apply_continuation_input(continuation, ContinuationInput::Return(value))
    }

    pub(crate) fn propagate_return_frame(
        &mut self,
        value: InterpValue,
        continuation: Continuation,
    ) -> ControlSignal {
        match continuation {
            Continuation::Chain { .. } => {
                unreachable!("EvalMachine must flatten continuation chains")
            }
            Continuation::CallBoundary { outer } => self.apply_continuation(*outer, value),
            continuation @ Continuation::AgentPromptBody { .. } => {
                self.apply_continuation(continuation, value)
            }
            Continuation::HandleBoundary { scope_id, span, .. } => {
                match self.close_handler_scope(scope_id, span) {
                    Ok(()) => ControlSignal::Return(value),
                    Err(fault) => ControlSignal::Fault(Box::new(fault)),
                }
            }
            Continuation::RetryAttempt { retry, .. } => {
                self.finish_retry_attempt_success(&retry);
                ControlSignal::Return(value)
            }
            Continuation::RestoreModelPolicy { previous, inner } => {
                self.model_policy = *previous;
                self.propagate_return_to_continuation(value, *inner)
            }
            Continuation::ScopedModelPolicy { policy, inner } => {
                let previous = std::mem::replace(&mut self.model_policy, *policy);
                self.propagate_return_to_continuation(
                    value,
                    compose_continuation(
                        *inner,
                        Continuation::RestoreModelPolicy {
                            previous: Box::new(previous),
                            inner: Box::new(Continuation::BlockValue),
                        },
                    ),
                )
            }
            _ => ControlSignal::Return(value),
        }
    }

    fn propagate_resume_to_continuation(
        &mut self,
        value: InterpValue,
        continuation: Continuation,
    ) -> ControlSignal {
        self.apply_continuation_input(continuation, ContinuationInput::Resume(value))
    }

    pub(crate) fn propagate_resume_frame(
        &mut self,
        value: InterpValue,
        continuation: Continuation,
    ) -> ControlSignal {
        match continuation {
            Continuation::Chain { .. } => {
                unreachable!("EvalMachine must flatten continuation chains")
            }
            Continuation::HandlerDispatch { outer } => self.apply_continuation(*outer, value),
            Continuation::HandleBoundary {
                scope_id,
                inner,
                handlers,
                span,
                mut frame,
            } => {
                if matches!(*inner, Continuation::BlockValue) {
                    self.continue_handle_signal(
                        ControlSignal::Resume(value),
                        scope_id,
                        handlers,
                        span,
                        &mut frame,
                    )
                } else {
                    let boundary = Continuation::HandleBoundary {
                        scope_id,
                        inner: Box::new(Continuation::BlockValue),
                        handlers,
                        span,
                        frame,
                    };
                    self.propagate_resume_to_continuation(
                        value,
                        compose_continuation(*inner, boundary),
                    )
                }
            }
            Continuation::RestoreModelPolicy { previous, inner } => {
                self.model_policy = *previous;
                self.propagate_resume_to_continuation(value, *inner)
            }
            _ => ControlSignal::Resume(value),
        }
    }

    fn propagate_finish_to_continuation(
        &mut self,
        value: InterpValue,
        continuation: Continuation,
    ) -> ControlSignal {
        self.apply_continuation_input(continuation, ContinuationInput::Finish(value))
    }

    pub(crate) fn propagate_finish_frame(
        &mut self,
        value: InterpValue,
        continuation: Continuation,
    ) -> ControlSignal {
        match continuation {
            Continuation::Chain { .. } => {
                unreachable!("EvalMachine must flatten continuation chains")
            }
            Continuation::HandlerDispatch { .. } => ControlSignal::Finish(value),
            Continuation::HandleBoundary { scope_id, span, .. } => {
                match self.close_handler_scope(scope_id, span) {
                    Ok(()) => ControlSignal::Value(value),
                    Err(fault) => ControlSignal::Fault(Box::new(fault)),
                }
            }
            Continuation::RetryAttempt { retry, .. } => {
                self.finish_retry_attempt_success(&retry);
                ControlSignal::Finish(value)
            }
            Continuation::RestoreModelPolicy { previous, inner } => {
                self.model_policy = *previous;
                self.propagate_finish_to_continuation(value, *inner)
            }
            _ => ControlSignal::Finish(value),
        }
    }

    pub(crate) fn propagate_loop_control_to_continuation(
        &mut self,
        signal: ControlSignal,
        continuation: Continuation,
    ) -> ControlSignal {
        debug_assert!(matches!(
            signal,
            ControlSignal::Break | ControlSignal::Continue
        ));
        let input = match signal {
            ControlSignal::Break => ContinuationInput::Break,
            ControlSignal::Continue => ContinuationInput::Continue,
            _ => unreachable!("loop-control propagation requires break or continue"),
        };
        self.apply_continuation_input(continuation, input)
    }

    pub(crate) fn propagate_loop_control_frame(
        &mut self,
        signal: ControlSignal,
        continuation: Continuation,
    ) -> ControlSignal {
        match continuation {
            Continuation::Chain { .. } => {
                unreachable!("EvalMachine must flatten continuation chains")
            }
            continuation @ (Continuation::ForLoop { .. } | Continuation::WhileLoop { .. }) => {
                match signal {
                    ControlSignal::Continue => {
                        self.apply_continuation(continuation, InterpValue::Unit)
                    }
                    ControlSignal::Break => ControlSignal::Value(InterpValue::Unit),
                    _ => unreachable!("loop-control propagation requires break or continue"),
                }
            }
            _ => signal,
        }
    }

    pub(super) fn apply_continuation(
        &mut self,
        continuation: Continuation,
        value: InterpValue,
    ) -> ControlSignal {
        self.apply_continuation_input(continuation, ContinuationInput::Value(value))
    }

    fn apply_continuation_input(
        &mut self,
        continuation: Continuation,
        input: ContinuationInput,
    ) -> ControlSignal {
        ControlSignal::pending_continuation(PendingContinuation {
            continuation,
            input,
        })
    }

    pub(crate) fn apply_continuation_frame(
        &mut self,
        continuation: Continuation,
        value: InterpValue,
    ) -> ControlSignal {
        match continuation {
            Continuation::ContinueBlock {
                block,
                next_stmt_index,
                mut frame,
            } => self.execute_block_from(block, next_stmt_index, &mut frame),
            Continuation::Bind {
                block,
                next_stmt_index,
                pat,
                span,
                mut frame,
            } => match self.bind_pattern(pat, value, &mut frame, span) {
                Ok(()) => self.execute_block_from(block, next_stmt_index, &mut frame),
                Err(fault) => ControlSignal::Fault(Box::new(fault)),
            },
            Continuation::Assign {
                block,
                next_stmt_index,
                target,
                span,
                mut frame,
            } => self
                .assign_target_in_block(block, next_stmt_index, target, value, &mut frame, span)
                .unwrap_or_else(|| self.execute_block_from(block, next_stmt_index, &mut frame)),
            Continuation::AssignTargetIndex {
                block,
                next_stmt_index,
                root_symbol,
                segments,
                components,
                next_component_index,
                new_value,
                span,
                mut frame,
            } => match self.finish_assign_target_index(
                crate::eval::assign_place::FinishAssignTargetIndex {
                    root_symbol,
                    segments,
                    components,
                    next_component_index,
                    index_value: value,
                    new_value,
                    block,
                    next_stmt_index,
                    span,
                },
                &mut frame,
            ) {
                Ok(()) => self.execute_block_from(block, next_stmt_index, &mut frame),
                Err(signal) => *signal,
            },
            Continuation::FieldReceiver {
                expr,
                field,
                span,
                frame: _frame,
            } => self.eval_field_value_signal(Some(expr), value, &field, span),
            Continuation::Unary { op, span } => match self.eval_unary_value(op, value, span) {
                Ok(value) => ControlSignal::Value(value),
                Err(fault) => ControlSignal::Fault(Box::new(fault)),
            },
            Continuation::BinaryLeft {
                op,
                rhs,
                span,
                mut frame,
            } => self.resume_binary_left(op, value, rhs, span, &mut frame),
            Continuation::BinaryRight { op, left, span } => {
                match self.finish_binary_right(op, left, value, span) {
                    Ok(value) => ControlSignal::Value(value),
                    Err(fault) => ControlSignal::Fault(Box::new(fault)),
                }
            }
            Continuation::AggregateElement {
                kind,
                exprs,
                next_index,
                mut values,
                mut frame,
            } => {
                values.push(value);
                self.resume_expr_sequence(kind, exprs, next_index, values, &mut frame)
            }
            Continuation::ListConsHead {
                tail,
                span,
                mut frame,
            } => self.resume_list_cons_head(value, tail, span, &mut frame),
            Continuation::ListConsTail { head, span } => self.finish_list_cons(head, value, span),
            Continuation::RangeStart {
                end,
                bounds,
                mut frame,
            } => self.resume_range_start(value, end, bounds, &mut frame),
            Continuation::RangeEnd { start, bounds } => self.finish_range(start, value, bounds),
            Continuation::RecordField {
                expr,
                nominal_type: _,
                variant_symbol: _,
                fields,
                next_index,
                mut values,
                mut frame,
            } => {
                let Some(etas_hir::HirFieldInit::Named { name, .. }) = next_index
                    .checked_sub(1)
                    .and_then(|index| fields.get(index))
                else {
                    return ControlSignal::missing_checked_fact(
                        "record field continuation does not point at a named field",
                        item_span(self.checked, self.entry_item),
                    );
                };
                values.push((name.clone(), value));
                self.resume_record_fields(expr, fields, next_index, values, &mut frame)
            }
            Continuation::MapKey {
                entries,
                index,
                values,
                mut frame,
            } => self.resume_map_key_value(entries, index, values, value, &mut frame),
            Continuation::MapValue {
                entries,
                index,
                key,
                values,
                mut frame,
            } => self.resume_map_value(entries, index, values, key, value, &mut frame),
            Continuation::IndexBase {
                expr,
                index,
                span,
                mut frame,
            } => self.resume_index_base(expr, value, index, span, &mut frame),
            Continuation::IndexValue { expr, base, span } => {
                self.eval_index_value(expr, base, value, span)
            }
            Continuation::SliceBase { eval, mut frame } => {
                self.resume_slice_base(eval, value, &mut frame)
            }
            Continuation::SliceStart {
                eval,
                base,
                mut frame,
            } => self.resume_slice_start(eval, base, value, &mut frame),
            Continuation::SliceEnd { eval, base, start } => {
                match self.eval_slice_value(eval.expr, base, start, value, eval.bounds, eval.span) {
                    Ok(value) => ControlSignal::Value(value),
                    Err(fault) => ControlSignal::Fault(Box::new(fault)),
                }
            }
            Continuation::MethodReceiver {
                expr,
                method,
                type_args,
                args,
                span,
                mut frame,
            } => self.eval_method_on_receiver(
                crate::eval::method::MethodDispatch {
                    expr,
                    method: &method,
                    type_args: &type_args,
                    args: &args,
                    span,
                },
                value,
                &mut frame,
            ),
            Continuation::PromptValueMethodArg {
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            } => self.finish_prompt_value_method_arg(
                messages,
                &method,
                role,
                allow_plain_system_content,
                value,
                span,
            ),
            Continuation::LocalMethodArgs {
                expr,
                receiver,
                method,
                type_args,
                args,
                next_arg_index,
                mut evaluated_args,
                span,
                mut frame,
            } => {
                evaluated_args.push(value);
                self.resume_local_method_args(
                    crate::eval::method::LocalMethodArgsState {
                        expr,
                        receiver,
                        method,
                        type_args,
                        args,
                        start_arg_index: next_arg_index,
                        evaluated_args,
                        span,
                    },
                    &mut frame,
                )
            }
            Continuation::StaticMethodArgs {
                expr,
                kind,
                method,
                type_args,
                args,
                next_arg_index,
                mut evaluated_args,
                span,
                mut frame,
            } => {
                evaluated_args.push(value);
                self.resume_static_method_args(
                    crate::eval::method::StaticMethodArgsState {
                        expr,
                        kind,
                        method,
                        type_args,
                        args,
                        start_arg_index: next_arg_index,
                        evaluated_args,
                        span,
                    },
                    &mut frame,
                )
            }
            Continuation::SpecMethodReceiver {
                expr: _,
                receiver_expr,
                spec_symbol,
                spec_args,
                method,
                args,
                span,
                mut frame,
            } => self.eval_spec_method_on_receiver_value(
                crate::eval::spec_method::SpecMethodDispatch {
                    receiver_expr,
                    receiver: value,
                    spec_symbol,
                    spec_args: &spec_args,
                    method: &method,
                    args: &args,
                    span,
                },
                &mut frame,
            ),
            Continuation::CalleeEval {
                args,
                span,
                mut frame,
            } => match value {
                InterpValue::Callable(target) => {
                    self.resume_call_args(target, args, 0, Vec::new(), span, &mut frame)
                }
                other => {
                    let message = format!("callee is not a callable runtime value: {:?}", other);
                    ControlSignal::invalid_arguments(message, span)
                }
            },
            Continuation::PipelineStageTarget {
                stages,
                next_stage_index,
                mut targets,
                current_limits,
                span,
                mut frame,
            } => match value {
                InterpValue::Callable(target) => {
                    targets.push(self.call_target_with_limits(target, current_limits));
                    self.resume_pipeline_stage_compose(
                        stages,
                        next_stage_index,
                        targets,
                        span,
                        &mut frame,
                    )
                }
                other => {
                    let message = format!(
                        "pipeline stage is not a callable runtime value: {:?}",
                        other
                    );
                    ControlSignal::invalid_arguments(message, span)
                }
            },
            Continuation::CallArgs {
                target,
                args,
                next_arg_index,
                mut evaluated_args,
                span,
                mut frame,
            } => {
                evaluated_args.push(value);
                self.resume_call_args(
                    target,
                    args,
                    next_arg_index,
                    evaluated_args,
                    span,
                    &mut frame,
                )
            }
            Continuation::VariantArgs {
                variant_symbol,
                args,
                next_arg_index,
                mut evaluated_args,
                span,
                mut frame,
            } => {
                evaluated_args.push(value);
                self.resume_variant_args(
                    variant_symbol,
                    args,
                    next_arg_index,
                    evaluated_args,
                    span,
                    &mut frame,
                )
            }
            Continuation::PerformArgs {
                expr,
                action,
                type_args,
                args,
                next_arg_index,
                mut evaluated_args,
                span,
                mut frame,
            } => {
                evaluated_args.push(value);
                self.resume_perform_args(
                    PerformArgsResume {
                        expr,
                        action,
                        type_args,
                        args,
                        start_arg_index: next_arg_index,
                        evaluated_args,
                        span,
                    },
                    &mut frame,
                )
            }
            Continuation::MemoryArgs {
                region_stable_id,
                path,
                key_type,
                value_type,

                result_type,
                method,
                args,
                next_arg_index,
                mut evaluated_args,
                span,
                mut frame,
            } => {
                evaluated_args.push(value);
                self.resume_memory_args(
                    MemoryArgsResume {
                        region_stable_id,
                        path,
                        key_type,
                        value_type,

                        result_type,
                        method,
                        args,
                        start_arg_index: next_arg_index,
                        evaluated_args,
                        span,
                    },
                    &mut frame,
                )
            }
            Continuation::MemorySelectionLimitArgs {
                region_stable_id,
                path,
                key_type,
                value_type,
                kind,
                predicate,
                limit,
                args,
                next_arg_index,
                mut evaluated_args,
                span,
                mut frame,
            } => {
                evaluated_args.push(value);
                self.resume_memory_selection_limit(
                    MemorySelectionLimitResume {
                        region_stable_id,
                        path,
                        key_type,
                        value_type,
                        kind,
                        predicate,
                        limit,
                        args,
                        start_arg_index: next_arg_index,
                        evaluated_args,
                        span,
                    },
                    &mut frame,
                )
            }
            Continuation::IfExpr {
                then_block,
                else_branch,
                span,
                mut frame,
            } => self.resume_if_expr(value, then_block, else_branch, span, &mut frame),
            Continuation::IfStmt {
                block,
                next_stmt_index,
                then_block,
                else_branch,
                span,
                mut frame,
            } => self.resume_if_stmt(
                IfStmtResume {
                    cond_value: value,
                    block,
                    next_stmt_index,
                    then_block,
                    else_branch,
                    span,
                },
                &mut frame,
            ),
            Continuation::MatchExpr {
                arms,
                span,
                mut frame,
            } => self.resume_match_expr(value, &arms, span, &mut frame),
            Continuation::MatchStmt {
                block,
                next_stmt_index,
                arms,
                span,
                mut frame,
            } => self.resume_match_stmt(value, block, next_stmt_index, &arms, span, &mut frame),
            Continuation::HandleHandler {
                handle_expr,
                body,
                handler,
                span,
                mut frame,
            } => self.resume_handle_handler(handle_expr, body, handler, span, value, &mut frame),
            Continuation::PipelineInput {
                stages,
                span,
                mut frame,
            } => self.resume_pipeline_input(value, &stages, span, &mut frame),
            Continuation::PipelineTarget { input, span } => match value {
                InterpValue::Callable(CallTarget::Composed(targets)) => {
                    self.execute_call_target(CallTarget::Composed(targets), vec![input], span)
                }
                other => {
                    let message = format!(
                        "pipeline stages did not evaluate to a composed callable: {other:?}"
                    );
                    ControlSignal::invalid_arguments(message, span)
                }
            },
            Continuation::ComposedCall { remaining, span } => {
                self.resume_composed_call(value, remaining, span)
            }
            Continuation::RestoreModelPolicy { previous, inner } => {
                self.model_policy = *previous;
                self.apply_continuation(*inner, value)
            }
            Continuation::CallBoundary { outer } => self.apply_continuation(*outer, value),
            Continuation::ForLoop {
                pat,
                values,
                next_index,
                body,
                iterations,
                loop_scope,
                span,
                mut frame,
            } => self.resume_for_loop(
                crate::eval::loop_control::ForLoopResume {
                    pat,
                    values,
                    next_index,
                    body,
                    iterations,
                    loop_scope,
                    span,
                },
                value,
                &mut frame,
            ),
            Continuation::WhileLoop {
                cond,
                body,
                iteration,
                max_iterations,
                resume_after_body,
                span,
                mut frame,
            } => self.resume_while_loop(
                crate::eval::loop_control::WhileLoopResume {
                    value,
                    cond,
                    body,
                    iteration,
                    max_iterations,
                    resume_after_body,
                    span,
                },
                &mut frame,
            ),
            Continuation::RetryAttempt {
                retry,
                body: _,
                attempts: _,
                next_attempt: _,
                block,
                next_stmt_index,
                mut frame,
            } => {
                if self
                    .retry_stack
                    .last()
                    .is_some_and(|current| current.id == retry.id)
                {
                    self.retry_stack.pop();
                }
                self.events
                    .push(WorkflowEvent::RetryAttemptSucceeded(retry.id));
                let _ = value;
                self.execute_block_from(block, next_stmt_index, &mut frame)
            }
            Continuation::TryExpr { expr, span } => self.resume_try_expr(expr, value, span),
            Continuation::MemoryClearDeleteAll {
                region_stable_id,
                path,
                span,
            } => match value {
                InterpValue::List(keys) => self.resume_memory_clear_delete_keys(
                    region_stable_id,
                    path,
                    keys.snapshot(),
                    0,
                    span,
                ),
                other => {
                    let message = format!(
                        "memory clear expected key-list result from scan, got {:?}",
                        other
                    );
                    ControlSignal::runtime_fault(message, span)
                }
            },
            Continuation::MemoryClearDeleteNext {
                region_stable_id,
                path,
                remaining_keys,
                next_index,
                span,
            } => self.resume_memory_clear_delete_keys(
                region_stable_id,
                path,
                remaining_keys,
                next_index,
                span,
            ),
            Continuation::HandlerDispatch { outer } => self.apply_continuation(*outer, value),
            Continuation::HandleBoundary {
                scope_id,
                inner,
                handlers,
                span,
                mut frame,
            } => {
                if matches!(*inner, Continuation::BlockValue) {
                    self.continue_handle_signal(
                        ControlSignal::Value(value),
                        scope_id,
                        handlers,
                        span,
                        &mut frame,
                    )
                } else {
                    let boundary = Continuation::HandleBoundary {
                        scope_id,
                        inner: Box::new(Continuation::BlockValue),
                        handlers,
                        span,
                        frame,
                    };
                    self.apply_continuation(compose_continuation(*inner, boundary), value)
                }
            }
            Continuation::AgentPromptBody {
                item,
                span,
                model_policy,
            } => match model_policy {
                Some(policy) => self.with_scoped_model_policy(*policy, |this| {
                    this.finish_agent_prompt_body(item, ControlSignal::Value(value), span)
                }),
                None => self.finish_agent_prompt_body(item, ControlSignal::Value(value), span),
            },
            Continuation::ScopedModelPolicy { policy, inner } => {
                let previous = std::mem::replace(&mut self.model_policy, *policy);
                self.apply_continuation(
                    compose_continuation(
                        *inner,
                        Continuation::RestoreModelPolicy {
                            previous: Box::new(previous),
                            inner: Box::new(Continuation::BlockValue),
                        },
                    ),
                    value,
                )
            }
            Continuation::Chain { .. } => {
                unreachable!("EvalMachine must flatten continuation chains")
            }
            Continuation::Return => ControlSignal::Return(value),
            Continuation::Resume => ControlSignal::Resume(value),
            Continuation::Finish => ControlSignal::Finish(value),
            Continuation::BlockValue => ControlSignal::Value(value),
        }
    }

    pub(super) fn finish_retry_attempt_success(
        &mut self,
        retry: &crate::orchestration::RetryAttemptRecord,
    ) {
        if self
            .retry_stack
            .last()
            .is_some_and(|current| current.id == retry.id)
        {
            self.retry_stack.pop();
        }
        self.events
            .push(WorkflowEvent::RetryAttemptSucceeded(retry.id));
    }

    fn with_scoped_model_policy(
        &mut self,
        policy: crate::api::ModelExecutionPolicy,
        f: impl FnOnce(&mut Self) -> ControlSignal,
    ) -> ControlSignal {
        let previous = std::mem::replace(&mut self.model_policy, policy.clone());
        let signal = f(self);
        self.model_policy = previous;
        self.scope_model_policy_signal(signal, policy)
    }

    pub(super) fn scope_model_policy_signal(
        &mut self,
        signal: ControlSignal,
        policy: crate::api::ModelExecutionPolicy,
    ) -> ControlSignal {
        match signal {
            ControlSignal::Apply(mut pending) => {
                pending.continuation =
                    scoped_model_policy_continuation(policy, pending.continuation);
                ControlSignal::Apply(pending)
            }
            ControlSignal::Expr(mut pending) => {
                pending.continuation =
                    scoped_model_policy_continuation(policy, pending.continuation);
                ControlSignal::Expr(pending)
            }
            ControlSignal::Call(mut pending) => {
                pending.continuation =
                    scoped_model_policy_continuation(policy, pending.continuation);
                ControlSignal::Call(pending)
            }
            ControlSignal::Memory(mut pending) => {
                pending.continuation =
                    scoped_model_policy_continuation(policy, pending.continuation);
                ControlSignal::Memory(pending)
            }
            ControlSignal::Session(mut pending) => {
                pending.continuation =
                    scoped_model_policy_continuation(policy, pending.continuation);
                ControlSignal::Session(pending)
            }
            ControlSignal::Perform(mut pending) => {
                pending.continuation =
                    scoped_model_policy_continuation(policy, pending.continuation);
                ControlSignal::Perform(pending)
            }
            ControlSignal::Console(mut pending) => {
                pending.continuation =
                    scoped_model_policy_continuation(policy, pending.continuation);
                ControlSignal::Console(pending)
            }
            ControlSignal::Command(mut pending) => {
                pending.continuation =
                    scoped_model_policy_continuation(policy, pending.continuation);
                ControlSignal::Command(pending)
            }
            ControlSignal::Model(mut pending) => {
                pending.continuation =
                    scoped_model_policy_continuation(policy, pending.continuation);
                ControlSignal::Model(pending)
            }
            ControlSignal::Host(mut pending) => {
                pending.continuation =
                    scoped_model_policy_continuation(policy, pending.continuation);
                ControlSignal::Host(pending)
            }
            other => other,
        }
    }
}

fn scoped_model_policy_continuation(
    policy: crate::api::ModelExecutionPolicy,
    inner: Continuation,
) -> Continuation {
    Continuation::ScopedModelPolicy {
        policy: Box::new(policy),
        inner: Box::new(inner),
    }
}

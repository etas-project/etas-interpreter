use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn eval_expr(&mut self, expr: HirExprId, frame: &mut Frame) -> ControlSignal {
        ControlSignal::pending_expr(PendingExpr {
            expr,
            frame: frame.clone(),
            continuation: Continuation::BlockValue,
        })
    }

    pub(crate) fn eval_expr_frame(&mut self, expr: HirExprId, frame: &mut Frame) -> ControlSignal {
        let span = self.checked.hir.exprs[expr].span(&self.checked.hir.blocks);
        if let Err(fault) = self.consume_execution_step(span) {
            return ControlSignal::Fault(Box::new(fault));
        }
        match &self.checked.hir.exprs[expr] {
            HirExpr::Literal(literal) => match self.eval_literal(expr, literal) {
                Ok(value) => ControlSignal::Value(value),
                Err(fault) => ControlSignal::Fault(Box::new(fault)),
            },
            HirExpr::Path(path) => {
                let signal = match self.eval_std_callable_value(expr, frame, path.span) {
                    Ok(Some(value)) => ControlSignal::Value(value),
                    Ok(None) => self.eval_path_signal(path.resolution.clone(), path.span, frame),
                    Err(fault) => ControlSignal::Fault(Box::new(fault)),
                };
                match signal {
                    ControlSignal::Value(InterpValue::Callable(target)) => {
                        match self.specialize_call_target_for_call(expr, target, frame, path.span) {
                            Ok(target) => ControlSignal::Value(InterpValue::Callable(target)),
                            Err(fault) => ControlSignal::Fault(Box::new(fault)),
                        }
                    }
                    other => other,
                }
            }
            HirExpr::Reflect(reflection) => self.eval_reflection(expr, reflection.span, frame),
            HirExpr::Field { base, field, span } => {
                self.eval_field(expr, *base, field, *span, frame)
            }
            HirExpr::Index { base, index, span } => {
                self.eval_index_expr(expr, *base, *index, *span, frame)
            }
            HirExpr::Slice {
                base,
                start,
                end,
                bounds,
                span,
            } => self.eval_slice_expr(
                SliceExprEval {
                    expr,
                    base: *base,
                    start: *start,
                    end: *end,
                    bounds: *bounds,
                    span: *span,
                },
                frame,
            ),
            HirExpr::Try {
                expr: operand,
                span,
            } => self.eval_try_expr(expr, *operand, *span, frame),
            HirExpr::Unary { op, expr, span } => self.eval_unary_expr(*op, *expr, *span, frame),
            HirExpr::Binary { op, lhs, rhs, span } => {
                self.eval_binary_expr(*op, *lhs, *rhs, *span, frame)
            }
            HirExpr::Record(record) => self.eval_record_expr(expr, record, frame),
            HirExpr::EmptyRecordOrMap { span } => self.eval_empty_record_or_map_expr(expr, *span),
            HirExpr::Tuple { .. } => self.eval_sequence_expr(expr, frame),
            HirExpr::Array { .. } => self.eval_sequence_expr(expr, frame),
            HirExpr::List { .. } => self.eval_sequence_expr(expr, frame),
            HirExpr::ListCons { head, tail, span } => {
                self.eval_list_cons_expr(*head, *tail, *span, frame)
            }
            HirExpr::EmptySequence { span } => self.eval_empty_sequence_expr(expr, *span),
            HirExpr::Map { .. } => self.eval_map_expr(expr, frame),
            HirExpr::Set { .. } => self.eval_sequence_expr(expr, frame),
            HirExpr::Range {
                start, end, bounds, ..
            } => self.eval_range_expr(*start, *end, *bounds, frame),
            HirExpr::Perform {
                action,
                generic_args,
                span,
                ..
            } => {
                let Some(type_args) = self.checked_type_args_from_generic_args(expr, generic_args)
                else {
                    return ControlSignal::missing_checked_fact(
                        "perform generic arguments require checked instantiation or selector facts",
                        *span,
                    );
                };
                let Some(args) = self.plan.arguments.get(expr) else {
                    return ControlSignal::missing_checked_fact(
                        "perform is missing its checked argument descriptors",
                        *span,
                    );
                };
                self.resume_perform_args(
                    PerformArgsResume {
                        expr,
                        action: action.clone(),
                        type_args,
                        args,
                        start_arg_index: 0,
                        evaluated_args: Vec::new(),
                        span: *span,
                    },
                    frame,
                )
            }
            HirExpr::Infer { span, .. } => self.eval_agent_inference(expr, frame, *span),
            HirExpr::Handler { handlers, span } => {
                self.eval_handler_value_expr(expr, handlers, *span)
            }
            HirExpr::Call {
                callee,
                generic_args,
                args,
                span,
            } => {
                let Some(type_args) = self.checked_type_args_from_generic_args(expr, generic_args)
                else {
                    return ControlSignal::missing_checked_fact(
                        "call generic arguments require checked instantiation facts",
                        *span,
                    );
                };
                if let Some(signal) = self.eval_static_collection_constructor_call(
                    expr, *callee, &type_args, args, *span, frame,
                ) {
                    return signal;
                }
                self.eval_call(expr, *callee, &type_args, args, *span, frame)
            }
            HirExpr::MethodCall {
                receiver,
                method,
                generic_args,
                args,
                span,
                ..
            } => {
                let Some(type_args) = self.checked_type_args_from_generic_args(expr, generic_args)
                else {
                    return ControlSignal::missing_checked_fact(
                        "method generic arguments require checked instantiation facts",
                        *span,
                    );
                };
                self.eval_method_call(
                    crate::eval::method::MethodDispatch {
                        expr,
                        method,
                        type_args: &type_args,
                        args,
                        span: *span,
                    },
                    *receiver,
                    frame,
                )
            }
            HirExpr::SpecMethodCall {
                receiver,
                spec_path,
                spec_args,
                method,
                args,
                span,
            } => self.eval_spec_method_call(
                crate::eval::spec_method::SpecMethodCall {
                    expr,
                    receiver_expr: *receiver,
                    spec_path,
                    spec_args,
                    method,
                    args,
                    span: *span,
                },
                frame,
            ),
            HirExpr::Handle {
                body,
                handler,
                span,
            } => self.eval_handle_expr(expr, *body, *handler, *span, frame),
            HirExpr::If {
                cond,
                then_block,
                else_branch,
                span,
            } => self.eval_if_expr(*cond, *then_block, else_branch, *span, frame),
            HirExpr::Match {
                scrutinee,
                arms,
                span,
            } => self.eval_match_expr(*scrutinee, arms, *span, frame),
            HirExpr::Lambda { .. } => self.eval_lambda_expr(expr, frame),
            HirExpr::StageCompose { span, .. } => self.eval_stage_compose_expr(expr, *span, frame),
            HirExpr::Pipeline { input, span, .. } => {
                self.eval_pipeline_expr(expr, *input, *span, frame)
            }
            HirExpr::Block(block) => self.execute_block(*block, frame),
            HirExpr::Error { span } => ControlSignal::fault(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                *span,
                "invalid checked-HIR error expression reached interpreter execution",
            ),
        }
    }

    pub(super) fn checked_type_args_from_generic_args(
        &self,
        expr: HirExprId,
        generic_args: &[HirGenericArg],
    ) -> Option<Vec<HirTypeId>> {
        let mut type_args = Vec::with_capacity(generic_args.len());
        for arg in generic_args {
            match arg {
                HirGenericArg::Type(ty) => type_args.push(*ty),
                HirGenericArg::EffectRow(_) => {
                    if self
                        .checked
                        .types
                        .generic_instantiations
                        .get(&expr)
                        .is_none_or(|fact| fact.effect_row_bindings.is_empty())
                    {
                        return None;
                    }
                }
                HirGenericArg::Wildcard { .. } => {
                    let callable = self
                        .checked
                        .types
                        .generic_instantiations
                        .get(&expr)
                        .is_some_and(|fact| {
                            !fact.type_bindings.is_empty() || !fact.effect_row_bindings.is_empty()
                        });
                    let selector = matches!(
                        self.checked.hir.exprs.get(expr),
                        Some(HirExpr::Perform { .. })
                    ) && self.checked.effects.performed_actions.contains_key(&expr);
                    if !callable && !selector {
                        return None;
                    }
                }
            }
        }
        Some(type_args)
    }
}

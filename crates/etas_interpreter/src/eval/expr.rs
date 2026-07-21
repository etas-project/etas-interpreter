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
            HirExpr::Path(path) => self.eval_path_signal(path.resolution.clone(), path.span, frame),
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
            HirExpr::Tuple { elems, .. } => self.eval_tuple_expr(elems, frame),
            HirExpr::Array { elems, .. } => self.eval_array_expr(elems, frame),
            HirExpr::List { elems, .. } => self.eval_list_expr(elems, frame),
            HirExpr::ListCons { head, tail, span } => {
                self.eval_list_cons_expr(*head, *tail, *span, frame)
            }
            HirExpr::EmptySequence { span } => self.eval_empty_sequence_expr(expr, *span),
            HirExpr::Map { entries, .. } => self.eval_map_expr(entries, frame),
            HirExpr::Set { elems, .. } => self.eval_set_expr(elems, frame),
            HirExpr::Range {
                start, end, bounds, ..
            } => self.eval_range_expr(*start, *end, *bounds, frame),
            HirExpr::Perform {
                action,
                generic_args,
                args,
                span,
            } => {
                let Some(type_args) = self.checked_type_args_from_generic_args(generic_args) else {
                    return ControlSignal::invalid_arguments("effect-row generic argument reached runtime perform dispatch without checked instantiation facts".to_owned(), *span);
                };
                self.resume_perform_args(
                    PerformArgsResume {
                        expr,
                        action: action.clone(),
                        type_args,
                        args: args.to_vec(),
                        start_arg_index: 0,
                        evaluated_args: Vec::new(),
                        span: *span,
                    },
                    frame,
                )
            }
            HirExpr::Handler { handlers, span } => {
                self.eval_handler_value_expr(expr, handlers, *span)
            }
            HirExpr::Call {
                callee,
                generic_args,
                args,
                span,
            } => {
                let Some(type_args) = self.checked_type_args_from_generic_args(generic_args) else {
                    return ControlSignal::invalid_arguments("effect-row generic argument reached runtime call dispatch without checked instantiation facts".to_owned(), *span);
                };
                if let Some(signal) =
                    self.eval_static_collection_constructor_call(*callee, &type_args, args, *span)
                {
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
                let Some(type_args) = self.checked_type_args_from_generic_args(generic_args) else {
                    return ControlSignal::invalid_arguments("effect-row generic argument reached runtime method dispatch without checked instantiation facts".to_owned(), *span);
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
            HirExpr::StageCompose { stages, span } => {
                self.eval_stage_compose_expr(stages, *span, frame)
            }
            HirExpr::Pipeline {
                input,
                stages,
                span,
            } => self.eval_pipeline_expr(*input, stages, *span, frame),
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
        generic_args: &[HirGenericArg],
    ) -> Option<Vec<HirTypeId>> {
        let mut type_args = Vec::with_capacity(generic_args.len());
        for arg in generic_args {
            match arg {
                HirGenericArg::Type(ty) => type_args.push(*ty),
                HirGenericArg::Wildcard { .. } | HirGenericArg::EffectRow(_) => return None,
            }
        }
        Some(type_args)
    }
}

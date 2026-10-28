use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn eval_stage_compose_expr(
        &mut self,
        expr: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        self.resume_pipeline_stage_compose(expr, 0, Vec::new(), span, frame)
    }

    pub(super) fn resume_pipeline_stage_compose(
        &mut self,
        expr: HirExprId,
        start_stage_index: usize,
        mut targets: Vec<CallTarget>,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let stages = match checked_pipeline_stages(self.checked, expr, span) {
            Ok(stages) => stages,
            Err(error) => return ControlSignal::missing_checked_fact(error, span),
        };
        for (index, stage) in stages.iter().enumerate().skip(start_stage_index) {
            for limit in &stage.limits {
                if let Err(fault) = self.resolve_limit_expr(*limit) {
                    return ControlSignal::Fault(Box::new(fault));
                }
            }
            let static_target = match self.resolve_static_call_target(stage.expr, frame, stage.span)
            {
                Ok(target) => target,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            };
            let target = if let Some(target) = static_target {
                target
            } else {
                match self.eval_expr(stage.expr, frame) {
                    ControlSignal::Value(InterpValue::Callable(target)) => target,
                    ControlSignal::Value(other) => {
                        let message =
                            format!("pipeline stage is not a callable runtime value: {other:?}");
                        return ControlSignal::invalid_arguments(message, stage.span);
                    }
                    signal @ (ControlSignal::Apply(_)
                    | ControlSignal::Checkpoint(_)
                    | ControlSignal::Block(_)
                    | ControlSignal::Expr(_)
                    | ControlSignal::Call(_)
                    | ControlSignal::Perform(_)
                    | ControlSignal::Memory(_)
                    | ControlSignal::Session(_)
                    | ControlSignal::Console(_)
                    | ControlSignal::Command(_)
                    | ControlSignal::Model(_)
                    | ControlSignal::Host(_)) => {
                        return compose_signal_continuation(
                            signal,
                            Continuation::PipelineStageTarget {
                                expr,
                                next_stage_index: index + 1,
                                targets,
                                span,
                                frame: frame.clone(),
                            },
                        );
                    }
                    ControlSignal::Return(value) => return ControlSignal::Return(value),
                    ControlSignal::Resume(value) => return ControlSignal::Resume(value),
                    ControlSignal::Finish(value) => return ControlSignal::Finish(value),
                    ControlSignal::Break => return ControlSignal::Break,
                    ControlSignal::Fault(fault) => return ControlSignal::Fault(fault),
                    ControlSignal::Cancelled(cause) => return ControlSignal::Cancelled(cause),
                    ControlSignal::Continue => return ControlSignal::Continue,
                }
            };
            targets.push(target);
        }
        if targets.len() != stages.len() {
            return ControlSignal::missing_checked_fact(
                "pipeline has an inconsistent resolved target count",
                span,
            );
        }
        let targets = targets
            .into_iter()
            .zip(stages)
            .map(|(target, stage)| {
                let limits = stage
                    .limits
                    .iter()
                    .map(|expr| self.resolve_limit_expr(*expr))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.call_target_with_limits(target, limits))
            })
            .collect::<Result<Vec<_>, crate::control::ExecutionFault>>();
        let targets = match targets {
            Ok(targets) => targets,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        ControlSignal::Value(InterpValue::Callable(CallTarget::Composed(targets.into())))
    }

    pub(super) fn call_target_with_limits(
        &self,
        target: CallTarget,
        limits: Vec<crate::eval::limit::RuntimeLimit>,
    ) -> CallTarget {
        if limits.is_empty() {
            target
        } else {
            CallTarget::Limited {
                target: target.into(),
                limits,
            }
        }
    }

    pub(super) fn eval_pipeline_expr(
        &mut self,
        expr: HirExprId,
        input: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let input = match self.eval_expr(input, frame) {
            ControlSignal::Value(value) => value,
            signal if is_pending_host_boundary_signal(&signal) => {
                return compose_signal_continuation(
                    signal,
                    Continuation::PipelineInput {
                        expr,
                        span,
                        frame: frame.clone(),
                    },
                );
            }
            other => return other,
        };
        self.resume_pipeline_input(expr, input, span, frame)
    }

    pub(super) fn resume_pipeline_input(
        &mut self,
        expr: HirExprId,
        input: InterpValue,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let composed = match self.eval_stage_compose_expr(expr, span, frame) {
            ControlSignal::Value(InterpValue::Callable(CallTarget::Composed(targets))) => targets,
            ControlSignal::Value(other) => {
                return ControlSignal::invalid_arguments(
                    format!("pipeline did not materialize a composed callable: {other:?}"),
                    span,
                );
            }
            signal if is_pending_host_boundary_signal(&signal) => {
                return compose_signal_continuation(
                    signal,
                    Continuation::PipelineTarget {
                        expr,
                        input,
                        span,
                        frame: frame.clone(),
                    },
                );
            }
            other => return other,
        };
        self.execute_call_target_at(
            CallTarget::Composed(composed),
            vec![input],
            span,
            Some(crate::control::CallSite {
                kind: crate::control::CallSiteKind::Pipeline { expr },
                frame: frame.clone(),
                composition: None,
            }),
        )
    }
}

pub(crate) fn checked_pipeline_stages(
    checked: &etas_frontend::CheckedProject,
    expr: HirExprId,
    span: Span,
) -> Result<&[etas_hir::HirStage], String> {
    match checked.hir.exprs.get(expr) {
        Some(
            HirExpr::StageCompose {
                stages,
                span: expected,
            }
            | HirExpr::Pipeline {
                stages,
                span: expected,
                ..
            },
        ) if *expected == span => Ok(stages),
        _ => Err("pipeline continuation has no matching checked owner".into()),
    }
}

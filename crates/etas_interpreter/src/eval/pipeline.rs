use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn eval_stage_compose_expr(
        &mut self,
        stages: &[etas_hir::HirStage],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        self.resume_pipeline_stage_compose(stages.to_vec(), 0, Vec::new(), span, frame)
    }

    pub(super) fn resume_pipeline_stage_compose(
        &mut self,
        stages: Vec<etas_hir::HirStage>,
        start_stage_index: usize,
        mut targets: Vec<CallTarget>,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        for (index, stage) in stages.iter().enumerate().skip(start_stage_index) {
            let mut limits = Vec::new();
            for limit in &stage.limits {
                let limit = match self.resolve_limit_expr(*limit) {
                    Ok(limit) => limit,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                };
                limits.push(limit);
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
                                stages: stages.clone(),
                                next_stage_index: index + 1,
                                targets: targets.clone(),
                                current_limits: limits,
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
                    ControlSignal::Continue => return ControlSignal::Continue,
                }
            };
            targets.push(self.call_target_with_limits(target, limits));
        }
        ControlSignal::Value(InterpValue::Callable(CallTarget::Composed(targets)))
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
                target: Box::new(target),
                limits,
            }
        }
    }

    pub(super) fn eval_pipeline_expr(
        &mut self,
        input: HirExprId,
        stages: &[etas_hir::HirStage],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let input = match self.eval_expr(input, frame) {
            ControlSignal::Value(value) => value,
            signal if is_pending_host_boundary_signal(&signal) => {
                return compose_signal_continuation(
                    signal,
                    Continuation::PipelineInput {
                        stages: stages.to_vec(),
                        span,
                        frame: frame.clone(),
                    },
                );
            }
            other => return other,
        };
        self.resume_pipeline_input(input, stages, span, frame)
    }

    pub(super) fn resume_pipeline_input(
        &mut self,
        input: InterpValue,
        stages: &[etas_hir::HirStage],
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        let composed = match self.eval_stage_compose_expr(stages, span, frame) {
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
                    Continuation::PipelineTarget { input, span },
                );
            }
            other => return other,
        };
        self.execute_call_target(CallTarget::Composed(composed), vec![input], span)
    }
}

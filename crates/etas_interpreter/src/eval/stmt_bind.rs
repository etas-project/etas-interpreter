use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn execute_var_stmt(
        &mut self,
        block: HirBlockId,
        stmt_index: usize,
        pat: etas_hir::HirPatId,
        value: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        let signal = self.eval_expr(value, frame);
        self.finish_bind_signal(block, stmt_index, pat, span, frame, signal)
    }

    pub(super) fn execute_let_stmt(
        &mut self,
        block: HirBlockId,
        stmt_index: usize,
        pat: etas_hir::HirPatId,
        value: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        let signal = self.eval_expr(value, frame);
        self.finish_bind_signal(block, stmt_index, pat, span, frame, signal)
    }

    fn finish_bind_signal(
        &mut self,
        block: HirBlockId,
        stmt_index: usize,
        pat: etas_hir::HirPatId,
        span: Span,
        frame: &mut Frame,
        signal: ControlSignal,
    ) -> Option<ControlSignal> {
        match signal {
            ControlSignal::Value(value) => match self.bind_pattern(pat, value, frame, span) {
                Ok(()) => None,
                Err(fault) => Some(ControlSignal::Fault(Box::new(fault))),
            },
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
            | ControlSignal::Host(_)) => Some(compose_signal_continuation(
                signal,
                Continuation::Bind {
                    block,
                    next_stmt_index: stmt_index + 1,
                    pat,
                    span,
                    frame: frame.clone(),
                },
            )),
            ControlSignal::Return(value) => Some(ControlSignal::Return(value)),
            ControlSignal::Resume(value) => Some(ControlSignal::Resume(value)),
            ControlSignal::Finish(value) => Some(ControlSignal::Finish(value)),
            ControlSignal::Break => Some(ControlSignal::Break),
            ControlSignal::Fault(fault) => Some(ControlSignal::Fault(fault)),
            ControlSignal::Cancelled(cause) => Some(ControlSignal::Cancelled(cause)),
            ControlSignal::Continue => Some(ControlSignal::Continue),
        }
    }
}

use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn execute_expr_stmt(
        &mut self,
        block: HirBlockId,
        stmt_index: usize,
        expr: HirExprId,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        let signal = self.eval_expr(expr, frame);
        self.finish_discard_signal(block, stmt_index, frame, signal)
    }

    pub(super) fn execute_return_stmt(
        &mut self,
        value: Option<HirExprId>,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        Some(match value {
            Some(expr) => {
                let signal = self.eval_expr(expr, frame);
                self.finish_exit_signal(signal, Continuation::Return)
            }
            None => ControlSignal::Return(InterpValue::Unit),
        })
    }

    pub(super) fn execute_resume_stmt(
        &mut self,
        value: Option<HirExprId>,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        Some(match value {
            Some(expr) => {
                let signal = self.eval_expr(expr, frame);
                self.finish_exit_signal(signal, Continuation::Resume)
            }
            None => ControlSignal::Resume(InterpValue::Unit),
        })
    }

    pub(super) fn execute_finish_stmt(
        &mut self,
        value: HirExprId,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        let signal = self.eval_expr(value, frame);
        Some(self.finish_exit_signal(signal, Continuation::Finish))
    }

    fn finish_discard_signal(
        &mut self,
        block: HirBlockId,
        stmt_index: usize,
        frame: &mut Frame,
        signal: ControlSignal,
    ) -> Option<ControlSignal> {
        match signal {
            ControlSignal::Value(_) => None,
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
                Continuation::ContinueBlock {
                    block,
                    next_stmt_index: stmt_index + 1,
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

    fn finish_exit_signal(
        &mut self,
        signal: ControlSignal,
        continuation: Continuation,
    ) -> ControlSignal {
        match signal {
            ControlSignal::Value(value) => match continuation {
                Continuation::Return => ControlSignal::Return(value),
                Continuation::Resume => ControlSignal::Resume(value),
                Continuation::Finish => ControlSignal::Finish(value),
                _ => unreachable!(),
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
            | ControlSignal::Host(_)) => compose_signal_continuation(signal, continuation),
            ControlSignal::Return(value) => ControlSignal::Return(value),
            ControlSignal::Resume(value) => ControlSignal::Resume(value),
            ControlSignal::Finish(value) => ControlSignal::Finish(value),
            ControlSignal::Break => ControlSignal::Break,
            ControlSignal::Fault(fault) => ControlSignal::Fault(fault),
            ControlSignal::Cancelled(cause) => ControlSignal::Cancelled(cause),
            ControlSignal::Continue => ControlSignal::Continue,
        }
    }
}

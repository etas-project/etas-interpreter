use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn execute_assign_stmt(
        &mut self,
        block: HirBlockId,
        stmt_index: usize,
        target: HirExprId,
        value: HirExprId,
        span: Span,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        let value = match self.eval_expr(value, frame) {
            ControlSignal::Cancelled(cause) => return Some(ControlSignal::Cancelled(cause)),
            ControlSignal::Value(value) => value,
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
                return Some(compose_signal_continuation(
                    signal,
                    Continuation::Assign {
                        block,
                        next_stmt_index: stmt_index + 1,
                        target,
                        span,
                        frame: frame.clone(),
                    },
                ));
            }
            ControlSignal::Return(value) => return Some(ControlSignal::Return(value)),
            ControlSignal::Resume(value) => return Some(ControlSignal::Resume(value)),
            ControlSignal::Finish(value) => return Some(ControlSignal::Finish(value)),
            ControlSignal::Break => return Some(ControlSignal::Break),
            ControlSignal::Fault(fault) => return Some(ControlSignal::Fault(fault)),
            ControlSignal::Continue => return Some(ControlSignal::Continue),
        };
        self.assign_target_in_block(block, stmt_index + 1, target, value, frame, span)
    }
}

use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn execute_if_stmt(
        &mut self,
        block: HirBlockId,
        stmt_index: usize,
        expr: HirExprId,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        let HirExpr::If {
            cond,
            then_block,
            else_branch,
            span,
        } = &self.checked.hir.exprs[expr]
        else {
            return Some(ControlSignal::missing_checked_fact(
                "if statement did not lower to a HIR if expression",
                self.checked.hir.exprs[expr].span(&self.checked.hir.blocks),
            ));
        };
        match self.eval_expr(*cond, frame) {
            ControlSignal::Value(value) => Some(self.resume_if_stmt(
                IfStmtResume {
                    cond_value: value,
                    block,
                    next_stmt_index: stmt_index + 1,
                    then_block: *then_block,
                    else_branch: else_branch.clone(),
                    span: *span,
                },
                frame,
            )),
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
                Continuation::IfStmt {
                    block,
                    next_stmt_index: stmt_index + 1,
                    then_block: *then_block,
                    else_branch: else_branch.clone(),
                    span: *span,
                    frame: frame.clone(),
                },
            )),
            ControlSignal::Return(value) => Some(ControlSignal::Return(value)),
            ControlSignal::Resume(value) => Some(ControlSignal::Resume(value)),
            ControlSignal::Finish(value) => Some(ControlSignal::Finish(value)),
            ControlSignal::Break => Some(ControlSignal::Break),
            ControlSignal::Fault(fault) => Some(ControlSignal::Fault(fault)),
            ControlSignal::Continue => Some(ControlSignal::Continue),
        }
    }

    pub(super) fn execute_match_stmt(
        &mut self,
        block: HirBlockId,
        stmt_index: usize,
        expr: HirExprId,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        let HirExpr::Match {
            scrutinee,
            arms,
            span,
        } = &self.checked.hir.exprs[expr]
        else {
            return Some(ControlSignal::missing_checked_fact(
                "match statement did not lower to a HIR match expression",
                self.checked.hir.exprs[expr].span(&self.checked.hir.blocks),
            ));
        };
        match self.eval_expr(*scrutinee, frame) {
            ControlSignal::Value(value) => {
                Some(self.resume_match_stmt(value, block, stmt_index + 1, arms, *span, frame))
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
            | ControlSignal::Host(_)) => Some(compose_signal_continuation(
                signal,
                Continuation::MatchStmt {
                    block,
                    next_stmt_index: stmt_index + 1,
                    arms: arms.clone(),
                    span: *span,
                    frame: frame.clone(),
                },
            )),
            ControlSignal::Return(value) => Some(ControlSignal::Return(value)),
            ControlSignal::Resume(value) => Some(ControlSignal::Resume(value)),
            ControlSignal::Finish(value) => Some(ControlSignal::Finish(value)),
            ControlSignal::Break => Some(ControlSignal::Break),
            ControlSignal::Fault(fault) => Some(ControlSignal::Fault(fault)),
            ControlSignal::Continue => Some(ControlSignal::Continue),
        }
    }
}

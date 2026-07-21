use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn execute_stmt(
        &mut self,
        block: HirBlockId,
        stmt_index: usize,
        stmt: etas_hir::HirStmtId,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        let span = match &self.checked.hir.stmts[stmt] {
            HirStmt::If(expr) | HirStmt::Match(expr) => {
                self.checked.hir.exprs[*expr].span(&self.checked.hir.blocks)
            }
            HirStmt::Let { span, .. }
            | HirStmt::Var { span, .. }
            | HirStmt::Assign { span, .. }
            | HirStmt::For { span, .. }
            | HirStmt::While { span, .. }
            | HirStmt::Retry { span, .. }
            | HirStmt::Resume { span, .. }
            | HirStmt::Finish { span, .. }
            | HirStmt::Return { span, .. }
            | HirStmt::Break { span }
            | HirStmt::Continue { span }
            | HirStmt::Expr { span, .. }
            | HirStmt::Error { span } => *span,
        };
        if let Err(fault) = self.consume_execution_step(span) {
            return Some(ControlSignal::Fault(Box::new(fault)));
        }
        match &self.checked.hir.stmts[stmt] {
            HirStmt::Var {
                pat, value, span, ..
            } => self.execute_var_stmt(block, stmt_index, *pat, *value, *span, frame),
            HirStmt::Let {
                pat, value, span, ..
            } => self.execute_let_stmt(block, stmt_index, *pat, *value, *span, frame),
            HirStmt::Expr { expr, .. } => self.execute_expr_stmt(block, stmt_index, *expr, frame),
            HirStmt::Assign {
                target,
                value,
                span,
            } => self.execute_assign_stmt(block, stmt_index, *target, *value, *span, frame),
            HirStmt::Return { value, .. } => self.execute_return_stmt(*value, frame),
            HirStmt::While {
                cond,
                limits,
                body,
                span,
            } => {
                let signal = self.execute_while_loop(*cond, limits, *body, *span, frame);
                match signal {
                    ControlSignal::Value(_) => None,
                    signal if is_pending_host_boundary_signal(&signal) => {
                        Some(compose_signal_continuation(
                            signal,
                            Continuation::ContinueBlock {
                                block,
                                next_stmt_index: stmt_index + 1,
                                frame: frame.clone(),
                            },
                        ))
                    }
                    other => Some(other),
                }
            }
            HirStmt::For {
                pat,
                iter,
                limits,
                body,
                span,
            } => {
                let signal = self.execute_for_loop(*pat, *iter, limits, *body, *span, frame);
                match signal {
                    ControlSignal::Value(_) => None,
                    signal if is_pending_host_boundary_signal(&signal) => {
                        Some(compose_signal_continuation(
                            signal,
                            Continuation::ContinueBlock {
                                block,
                                next_stmt_index: stmt_index + 1,
                                frame: frame.clone(),
                            },
                        ))
                    }
                    other => Some(other),
                }
            }
            HirStmt::Retry { body, limits, span } => {
                self.execute_retry_stmt(block, stmt_index, *body, limits, *span, frame)
            }
            HirStmt::Resume { value, .. } => self.execute_resume_stmt(*value, frame),
            HirStmt::Finish { value, .. } => self.execute_finish_stmt(*value, frame),
            HirStmt::Error { span } | HirStmt::Break { span } | HirStmt::Continue { span } => {
                match &self.checked.hir.stmts[stmt] {
                    HirStmt::Break { .. } => Some(ControlSignal::Break),
                    HirStmt::Continue { .. } => Some(ControlSignal::Continue),
                    _ => self.unsupported_stmt(*span),
                }
            }
            HirStmt::If(expr) => self.execute_if_stmt(block, stmt_index, *expr, frame),
            HirStmt::Match(expr) => self.execute_match_stmt(block, stmt_index, *expr, frame),
        }
    }

    fn unsupported_stmt(&mut self, span: Span) -> Option<ControlSignal> {
        Some(ControlSignal::invalid_arguments(
            "invalid checked-HIR statement reached interpreter execution".to_owned(),
            span,
        ))
    }
}

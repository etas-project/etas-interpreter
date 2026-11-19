use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn execute_block(&mut self, block: HirBlockId, frame: &mut Frame) -> ControlSignal {
        self.execute_block_from(block, 0, frame)
    }

    pub(super) fn execute_block_from(
        &mut self,
        block: HirBlockId,
        start_stmt_index: usize,
        frame: &mut Frame,
    ) -> ControlSignal {
        ControlSignal::pending_block(PendingBlock {
            agent_context: None,
            block,
            next_stmt_index: start_stmt_index,
            frame: frame.clone(),
            continuation: Continuation::BlockValue,
            residual: None,
        })
    }

    pub(crate) fn execute_block_frame(
        &mut self,
        block: HirBlockId,
        start_stmt_index: usize,
        frame: &mut Frame,
    ) -> ControlSignal {
        let block_data = &self.checked.hir.blocks[block];
        for (index, stmt) in block_data.stmts.iter().enumerate().skip(start_stmt_index) {
            if let Some(signal) = self.execute_stmt(block, index, *stmt, frame) {
                return signal;
            }
        }
        if let Some(expr) = block_data.final_expr {
            let signal = match self.eval_expr(expr, frame) {
                ControlSignal::Perform(mut perform) => {
                    perform.continuation =
                        compose_continuation(perform.continuation, Continuation::BlockValue);
                    ControlSignal::Perform(perform)
                }
                other => other,
            };
            signal
        } else {
            ControlSignal::Value(InterpValue::Unit)
        }
    }

    pub(crate) fn finish_scope_signal(
        &mut self,
        block: HirBlockId,
        frame: &mut Frame,
        mut scope_snapshot: std::rc::Rc<std::collections::HashSet<SymbolId>>,
        guard: Option<std::rc::Rc<crate::control::ScopeExitGuard>>,
        signal: ControlSignal,
    ) -> ControlSignal {
        if let Some(guard) = &guard {
            guard.close();
        }
        let block_data = &self.checked.hir.blocks[block];
        // Keep the actual entry baseline, including optimizer-hoisted lets.
        // A captured var still ends at its declaring block after suspension.
        for symbol in &self
            .checked
            .hir
            .scopes
            .get(block_data.scope)
            .expect("checked block scope")
            .symbols
        {
            if matches!(
                self.checked.types.symbol_types.get(symbol),
                Some(etas_types::SymbolTypeFact::Local { mutable: true, .. })
            ) && scope_snapshot.contains(symbol)
            {
                std::rc::Rc::make_mut(&mut scope_snapshot).remove(symbol);
            }
        }
        let escapes = match &signal {
            ControlSignal::Value(value)
            | ControlSignal::Return(value)
            | ControlSignal::Resume(value)
            | ControlSignal::Finish(value) => frame.scoped_value_escapes(value, &scope_snapshot),
            ControlSignal::Perform(pending) => pending
                .args
                .iter()
                .any(|value| frame.scoped_value_escapes(value, &scope_snapshot)),
            _ => false,
        } || frame.scoped_local_escapes(&scope_snapshot);
        frame.cleanup_to(&scope_snapshot);
        if escapes {
            return ControlSignal::invalid_arguments(
                "lambda or Handler retaining a local var cannot leave its owning scope",
                block_data.span,
            );
        }
        signal
    }
}

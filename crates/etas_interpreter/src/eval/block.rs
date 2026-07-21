use std::collections::HashSet;

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
            block,
            next_stmt_index: start_stmt_index,
            frame: frame.clone(),
            continuation: Continuation::BlockValue,
        })
    }

    pub(crate) fn execute_block_frame(
        &mut self,
        block: HirBlockId,
        start_stmt_index: usize,
        frame: &mut Frame,
    ) -> ControlSignal {
        let scope_snapshot = frame.snapshot_symbols();
        let block_data = &self.checked.hir.blocks[block];
        for (index, stmt) in block_data.stmts.iter().enumerate().skip(start_stmt_index) {
            if let Some(signal) = self.execute_stmt(block, index, *stmt, frame) {
                return self.finish_block_signal(signal, frame, &scope_snapshot);
            }
        }
        if let Some(expr) = block_data.final_expr {
            if let Some(label) = self.checkpoint_label(expr, frame) {
                return self.finish_block_signal(
                    ControlSignal::pending_checkpoint(PendingCheckpoint {
                        label,
                        continuation: Continuation::BlockValue,
                    }),
                    frame,
                    &scope_snapshot,
                );
            }
            let signal = match self.eval_expr(expr, frame) {
                ControlSignal::Perform(mut perform) => {
                    perform.continuation =
                        compose_continuation(perform.continuation, Continuation::BlockValue);
                    ControlSignal::Perform(perform)
                }
                other => other,
            };
            self.finish_block_signal(signal, frame, &scope_snapshot)
        } else {
            self.finish_block_signal(
                ControlSignal::Value(InterpValue::Unit),
                frame,
                &scope_snapshot,
            )
        }
    }

    fn finish_block_signal(
        &mut self,
        signal: ControlSignal,
        frame: &mut Frame,
        scope_snapshot: &HashSet<SymbolId>,
    ) -> ControlSignal {
        if !is_pending_host_boundary_signal(&signal) {
            frame.cleanup_to(scope_snapshot);
        }
        signal
    }
}

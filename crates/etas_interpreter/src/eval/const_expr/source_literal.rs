use super::*;
use crate::eval::machine::{EvalMachine, MachinePoll};

impl EvalContext<'_> {
    pub(super) fn eval_const_source_literal(
        &mut self,
        expr: HirExprId,
        span: Span,
    ) -> Result<InterpValue, ConstantFailure> {
        let program = self.checked_execution_program();
        let handler_floor = self.handler_floor;
        let signal = self.eval_expr(expr, &mut Frame::new(std::sync::Arc::default()));
        let mut machine = EvalMachine::pure_signal(signal, span);
        loop {
            let scope = machine.program_scope();
            let owner = scope.as_ref().map_or(&program, |scope| &scope.program);
            let floor = scope
                .as_ref()
                .map_or(handler_floor, |scope| scope.handler_floor);
            let mut scoped = self.scoped_context(owner, floor);
            match machine.run_until_yield(&mut scoped) {
                MachinePoll::Complete(value) => return Ok(*value),
                MachinePoll::CooperativeYield | MachinePoll::ProgramChanged => continue,
                MachinePoll::Fault(fault) => return Err(fault.into()),
                MachinePoll::Cancelled(cause) => return Err(ConstantFailure::Cancelled(cause)),
                MachinePoll::Yield(_) => {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        "constant collection constructor attempted to suspend",
                    )
                    .into());
                }
            }
        }
    }
}

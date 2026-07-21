use etas_core::{AnalysisDiagnosticCode, Span};

use crate::{control::ExecutionFault, eval::EvalContext};

use super::state::EvalMachine;

pub(super) fn check_call_budget(
    ctx: &EvalContext<'_>,
    machine: &EvalMachine,
    span: Span,
) -> Result<(), ExecutionFault> {
    let max_call_depth = ctx.execution_limits.max_call_depth.get();
    let call_depth = machine.active_call_depth();
    if call_depth < max_call_depth {
        return Ok(());
    }
    let message = format!(
        "maximum interpreter call depth ({max_call_depth}) exceeded while executing callable; this usually indicates unbounded recursion or a callable resolution cycle"
    );
    Err(ExecutionFault::new(
        AnalysisDiagnosticCode::UnhandledRuntimeError,
        span,
        message,
    ))
}

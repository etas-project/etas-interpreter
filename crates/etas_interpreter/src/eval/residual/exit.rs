use etas_code::residual::monitor::ScopeCheckResult;
use etas_core::{AnalysisDiagnosticCode, Span};

use crate::control::{
    ControlSignal, ExecutionFault,
    residual::{ResidualExitKind, SharedResidualScope},
};
use crate::eval::EvalContext;

impl EvalContext<'_> {
    /// Validates a scope at an exit. A normal return releases its result only
    /// when every monitor accepts; abnormal exits are recorded as incomplete
    /// and never as satisfied, and keep their primary cause.
    pub(crate) fn exit_residual_scope(
        &mut self,
        scope: &SharedResidualScope,
        kind: ResidualExitKind,
        span: Span,
    ) -> Result<(), ExecutionFault> {
        let fail = |reason: String| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::TraceSpecResidualViolation,
                span,
                reason,
            )
        };
        let mut scope = scope
            .try_borrow_mut()
            .map_err(|_| fail("residual scope is busy at exit".into()))?;
        let span = scope.span();
        let fail = |reason: String| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::TraceSpecResidualViolation,
                span,
                reason,
            )
        };
        let result = scope.finish(kind).map_err(fail)?;
        if kind == ResidualExitKind::Return && result != ScopeCheckResult::Accepting {
            return Err(fail(format!(
                "trace spec residual check did not accept the completed scope: {result:?}"
            )));
        }
        Ok(())
    }
}

impl EvalContext<'_> {
    /// Exits a captured scope on a control path and continues with `signal`.
    pub(crate) fn residual_exit_signal(
        &mut self,
        scope: &SharedResidualScope,
        kind: ResidualExitKind,
        signal: ControlSignal,
    ) -> ControlSignal {
        let span = crate::diagnostics::item_span(self.checked, self.entry_item);
        match self.exit_residual_scope(scope, kind, span) {
            Ok(()) => signal,
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        }
    }
}

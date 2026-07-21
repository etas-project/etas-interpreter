use etas_core::{AnalysisDiagnosticCode, Diagnostic, Span};

use crate::{
    control::{Continuation, ControlSignal},
    eval::{EvalContext, machine::EvalMachine},
};

pub(in crate::driver) fn format_host_error(error: &etas_host::HostError) -> String {
    if error.details.is_empty() {
        return error.message.clone();
    }
    let details = error
        .details
        .iter()
        .map(|detail| format!("{}={}", detail.key, detail.value))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{} ({details})", error.message)
}

pub(in crate::driver) fn retry_or_report(
    eval: &mut EvalContext<'_>,
    machine: &mut EvalMachine,
    continuation: Continuation,
    span: Span,
    message: String,
) -> Option<ControlSignal> {
    if let Some(signal) = machine.retry_boundary_failure(eval, continuation, span, message.clone())
    {
        return Some(signal);
    }
    eval.diagnostics.push(Diagnostic::analysis(
        AnalysisDiagnosticCode::UnhandledRuntimeError,
        span,
        message,
    ));
    None
}

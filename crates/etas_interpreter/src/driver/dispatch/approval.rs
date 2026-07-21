use etas_core::{AnalysisDiagnosticCode, Diagnostic};

use crate::{
    control::{ControlSignal, PendingPerform},
    eval::{EvalContext, machine::EvalMachine},
    host::HostServices,
    value::InterpValue,
};

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    perform: PendingPerform,
    machine: &mut EvalMachine,
) -> Option<ControlSignal> {
    if let Some(value) = eval.replayed_approval_result(&perform) {
        return Some(eval.resume_perform_signal(perform, value));
    }
    if perform.error_type.is_some() {
        let diagnostic = crate::diagnostics::unhandled_error_perform(eval.checked, &perform);
        if let Some(signal) = machine.retry_boundary_failure(
            eval,
            perform.continuation.clone(),
            perform.span,
            diagnostic.message.clone(),
        ) {
            return Some(signal);
        }
        eval.diagnostics.push(diagnostic);
        return None;
    }
    let Some((key, request)) = eval.approval_request_for(&perform) else {
        eval.diagnostics
            .push(crate::diagnostics::unhandled_effect_action(&perform));
        return None;
    };
    eval.record_host_request_sent(request.id);
    let request_id = request.id;
    match host.approval(request).await {
        Ok(decision) => {
            eval.record_host_response_received(request_id);
            let value = match decision {
                etas_host::ApprovalDecision::Approved { .. } => InterpValue::Bool(true),
                etas_host::ApprovalDecision::Denied { .. } => InterpValue::Bool(false),
            };
            eval.record_completed_host_boundary("approval", key, value.clone());
            Some(eval.resume_perform_signal(perform, value))
        }
        Err(error) => {
            eval.record_host_response_received(request_id);
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                perform.span,
                format!("approval host boundary failed: {}", error.message),
            ));
            None
        }
    }
}

use etas_core::{AnalysisDiagnosticCode, Diagnostic};

use crate::{
    control::{ControlSignal, PendingPerform},
    eval::{EvalContext, machine::EvalMachine},
    host::HostServices,
    value::InterpValue,
};

use super::host_dispatch::HostDispatch;

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
    if let Err(error) = eval.host_budget().check_time() {
        eval.diagnostics.push(Diagnostic::analysis(
            AnalysisDiagnosticCode::UnhandledRuntimeError,
            perform.span,
            format!("approval host boundary failed: {}", error.message),
        ));
        return None;
    }
    let authority = eval.host_authority();
    match HostDispatch::execute_approval(eval, request.clone(), authority, host.approval(request))
        .await
    {
        Ok(response) => {
            let value = match response.decision {
                etas_host::ApprovalDecision::Approved { .. } => InterpValue::Bool(true),
                etas_host::ApprovalDecision::Denied { .. } => InterpValue::Bool(false),
            };
            eval.record_completed_host_boundary("approval", key, value.clone());
            Some(eval.resume_perform_signal(perform, value))
        }
        Err(error) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                perform.span,
                format!("approval host boundary failed: {}", error.message),
            ));
            None
        }
    }
}

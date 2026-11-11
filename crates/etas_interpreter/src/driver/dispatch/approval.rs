use etas_core::{AnalysisDiagnosticCode, Diagnostic};

use crate::{
    control::{ControlSignal, PendingPerform},
    eval::{EvalContext, machine::EvalMachine},
    host::HostServices,
    value::InterpValue,
};

use super::host_dispatch::{ApprovalCall, HostCompletion, HostDispatch, PendingHostOperation};

pub(in crate::driver) enum ApprovalDispatchStart<'host> {
    Complete(Option<ControlSignal>),
    Pending(Box<PendingApprovalDispatch<'host>>),
}

pub(in crate::driver) struct PendingApprovalDispatch<'host> {
    perform: PendingPerform,
    key: etas_host::replay::ReplayFingerprint,
    request_id: etas_host::HostRequestId,
    pending: PendingHostOperation<etas_host::ApprovalResponse, ApprovalCall<'host>>,
}

pub(in crate::driver) struct CompletedApprovalDispatch {
    perform: PendingPerform,
    key: etas_host::replay::ReplayFingerprint,
    request_id: etas_host::HostRequestId,
    completion: HostCompletion<etas_host::ApprovalResponse>,
}

pub(in crate::driver) fn begin<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    perform: PendingPerform,
    machine: &mut EvalMachine,
) -> ApprovalDispatchStart<'host> {
    if perform.error_type.is_some() {
        let diagnostic = crate::diagnostics::unhandled_error_perform(eval.checked, &perform);
        if let Some(signal) = machine.retry_boundary_failure(
            eval,
            perform.continuation.clone(),
            perform.span,
            diagnostic.message.clone(),
        ) {
            return ApprovalDispatchStart::Complete(Some(signal));
        }
        eval.diagnostics.push(diagnostic);
        return ApprovalDispatchStart::Complete(None);
    }
    let (key, request) = match eval.approval_request_for(&perform) {
        Ok(Some(request)) => request,
        Ok(None) => {
            eval.diagnostics
                .push(crate::diagnostics::unhandled_effect_action(&perform));
            return ApprovalDispatchStart::Complete(None);
        }
        Err(fault) => {
            return ApprovalDispatchStart::Complete(Some(ControlSignal::Fault(Box::new(fault))));
        }
    };
    match eval.completed_host_boundary_result(
        &crate::orchestration::BoundaryOccurrenceId::HostRequest(request.id),
        "approval",
        &key,
        perform.span,
    ) {
        Ok(Some(value)) => {
            return ApprovalDispatchStart::Complete(Some(
                eval.resume_perform_signal(perform, value),
            ));
        }
        Ok(None) => (),
        Err(fault) => {
            return ApprovalDispatchStart::Complete(Some(ControlSignal::Fault(Box::new(fault))));
        }
    }
    let request_id = request.id;
    if let Err(error) = eval.host_budget().check_time() {
        eval.diagnostics.push(Diagnostic::analysis(
            AnalysisDiagnosticCode::UnhandledRuntimeError,
            perform.span,
            format!("approval host boundary failed: {}", error.message),
        ));
        return ApprovalDispatchStart::Complete(None);
    }
    let authority = eval.host_authority();
    match HostDispatch::begin_approval(eval, request.clone(), authority, |operation| {
        host.approval(operation, request)
    }) {
        Ok(pending) => ApprovalDispatchStart::Pending(Box::new(PendingApprovalDispatch {
            perform,
            key,
            request_id,
            pending,
        })),
        Err(error) => ApprovalDispatchStart::Complete(finish_error(eval, perform, error)),
    }
}

pub(in crate::driver) fn finish(
    eval: &mut EvalContext<'_>,
    completed: CompletedApprovalDispatch,
) -> Option<ControlSignal> {
    let CompletedApprovalDispatch {
        perform,
        key,
        request_id,
        completion,
    } = completed;
    match HostDispatch::finish(eval, completion) {
        Ok(response) => {
            let approved = matches!(
                response.decision,
                etas_host::ApprovalDecision::Approved { .. }
            );
            let value = InterpValue::Bool(approved);
            if let Some(tag) = &perform.residual {
                tag.record_trusted_approval(approved);
            }
            if let Err(fault) = eval.record_completed_host_boundary(
                crate::orchestration::BoundaryOccurrenceId::HostRequest(request_id),
                "approval",
                key,
                value.clone(),
                perform.span,
            ) {
                return Some(ControlSignal::Fault(Box::new(fault)));
            }
            Some(eval.resume_perform_signal(perform, value))
        }
        Err(error) => finish_error(eval, perform, error),
    }
}

fn finish_error(
    eval: &mut EvalContext<'_>,
    perform: PendingPerform,
    error: etas_host::HostError,
) -> Option<ControlSignal> {
    if let Some(signal) = eval.cancellation_signal(perform.span) {
        return Some(signal);
    }
    eval.diagnostics.push(Diagnostic::analysis(
        AnalysisDiagnosticCode::UnhandledRuntimeError,
        perform.span,
        format!("approval host boundary failed: {}", error.message),
    ));
    None
}

impl PendingApprovalDispatch<'_> {
    pub(in crate::driver) fn operation_id(&self) -> etas_host::execution::OperationId {
        self.pending.operation_id()
    }

    pub(in crate::driver) async fn poll(self) -> CompletedApprovalDispatch {
        CompletedApprovalDispatch {
            perform: self.perform,
            key: self.key,
            request_id: self.request_id,
            completion: HostDispatch::poll(self.pending).await,
        }
    }
}

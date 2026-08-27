use etas_core::{AnalysisDiagnosticCode, Diagnostic, Span};
use etas_host::{
    HostRequestKind, HostTraceRequest, HostValue, PolicyDecision, PolicyEvaluationRequest,
    PolicySubject,
};

use crate::{eval::EvalContext, host::HostServices};

use super::host_dispatch::HostDispatch;

pub(in crate::driver) fn boundary_policy_ref_for(
    eval: &EvalContext<'_>,
    explicit: Option<HostValue>,
) -> Option<HostValue> {
    explicit.or_else(|| eval.boundary_policy_ref())
}

pub(in crate::driver) async fn evaluate_before_boundary(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    policy_ref: Option<HostValue>,
    mut subject: PolicySubject,
    span: Span,
    boundary: &'static str,
) -> bool {
    if let Err(error) = eval.host_budget().check_time() {
        eval.diagnostics.push(Diagnostic::analysis(
            AnalysisDiagnosticCode::UnhandledRuntimeError,
            span,
            format!("{boundary} policy boundary failed: {}", error.message),
        ));
        return false;
    }
    let policy_ref = match policy_ref {
        Some(policy_ref) => policy_ref,
        None => {
            let policy = &eval.host_context.authority.policy;
            if policy.active_trace_specs.is_empty() && policy.trace_spec_facts.is_empty() {
                return true;
            }
            HostValue::String(etas_host::TRACE_SPEC_RUNTIME_REF.to_owned())
        }
    };
    let policy_request_id = eval.next_host_request_id();
    let trace = eval.host_trace();
    subject.attributes.push((
        "trace_id".to_owned(),
        HostValue::String(format!("{:?}", trace.trace_id)),
    ));
    let policy_request = PolicyEvaluationRequest {
        id: policy_request_id,
        policy_ref,
        subject,
        authority: eval.host_authority(),
        trace,
    };
    let policy_authority = policy_request.authority.clone();
    let policy_trace = policy_request.trace.clone();
    let trace_payload = policy_request.trace_payload();
    match HostDispatch::execute(
        eval,
        policy_request_id,
        HostRequestKind::Policy,
        trace_payload,
        policy_authority,
        policy_trace,
        host.policy(policy_request),
    )
    .await
    {
        Ok(response) => match response.decision {
            PolicyDecision::Allow => true,
            PolicyDecision::Deny { reason } => {
                eval.diagnostics.push(Diagnostic::analysis(
                    AnalysisDiagnosticCode::UnhandledRuntimeError,
                    span,
                    format!("{boundary} policy denied request: {reason}"),
                ));
                false
            }
            PolicyDecision::RequireApproval { request } => {
                if let Err(error) = eval.host_budget().check_time() {
                    eval.diagnostics.push(Diagnostic::analysis(
                        AnalysisDiagnosticCode::UnhandledRuntimeError,
                        span,
                        format!("{boundary} approval boundary failed: {}", error.message),
                    ));
                    return false;
                }
                let authority = eval.host_authority();
                match HostDispatch::execute_approval(
                    eval,
                    request.clone(),
                    authority,
                    host.approval(request),
                )
                .await
                {
                    Ok(response) => match response.decision {
                        etas_host::ApprovalDecision::Approved { grant } => {
                            eval.record_approval_grant(grant);
                            true
                        }
                        etas_host::ApprovalDecision::Denied { .. } => {
                            eval.diagnostics.push(Diagnostic::analysis(
                                AnalysisDiagnosticCode::UnhandledRuntimeError,
                                span,
                                format!("{boundary} policy approval was denied"),
                            ));
                            false
                        }
                    },
                    Err(error) => {
                        eval.diagnostics.push(Diagnostic::analysis(
                            AnalysisDiagnosticCode::UnhandledRuntimeError,
                            span,
                            format!("policy approval host boundary failed: {}", error.message),
                        ));
                        false
                    }
                }
            }
        },
        Err(error) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                format!("policy host boundary failed: {}", error.message),
            ));
            false
        }
    }
}

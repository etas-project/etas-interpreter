use etas_core::{AnalysisDiagnosticCode, Diagnostic, Span};
use etas_host::{HostValue, PolicyDecision, PolicyEvaluationRequest, PolicySubject};

use crate::{eval::EvalContext, host::HostServices};

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
    eval.record_host_request_sent(policy_request_id);
    match host.policy(policy_request).await {
        Ok(response) => {
            eval.record_host_response_received(response.id);
            match response.decision {
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
                    let approval_id = request.id;
                    eval.record_host_request_sent(approval_id);
                    match host.approval(request).await {
                        Ok(decision) => {
                            eval.record_host_response_received(approval_id);
                            match decision {
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
                            }
                        }
                        Err(error) => {
                            eval.record_host_response_received(approval_id);
                            eval.diagnostics.push(Diagnostic::analysis(
                                AnalysisDiagnosticCode::UnhandledRuntimeError,
                                span,
                                format!("policy approval host boundary failed: {}", error.message),
                            ));
                            false
                        }
                    }
                }
            }
        }
        Err(error) => {
            eval.record_host_response_received(policy_request_id);
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                format!("policy host boundary failed: {}", error.message),
            ));
            false
        }
    }
}

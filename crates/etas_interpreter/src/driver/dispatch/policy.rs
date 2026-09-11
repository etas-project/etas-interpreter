use etas_core::{AnalysisDiagnosticCode, Diagnostic, Span};
use etas_host::{
    HostError, HostErrorCode, HostRequestKind, HostTraceRequest, HostValue, PolicyDecision,
    PolicyEvaluationRequest, PolicySubject,
};

use super::host_dispatch::HostDispatch;
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
    subject: PolicySubject,
    span: Span,
    boundary: &'static str,
) -> bool {
    match authorize_before_boundary(eval, host, policy_ref, subject).await {
        Ok(()) => true,
        Err(error) => {
            if eval.cancellation_signal(span).is_none() {
                eval.diagnostics.push(Diagnostic::analysis(
                    AnalysisDiagnosticCode::UnhandledRuntimeError,
                    span,
                    policy_failure_message(boundary, &error),
                ));
            }
            false
        }
    }
}

/// Policy decisions are data; the caller chooses its declared error contract.
pub(in crate::driver) async fn authorize_before_boundary(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    policy_ref: Option<HostValue>,
    mut subject: PolicySubject,
) -> Result<(), HostError> {
    eval.host_budget().check_time()?;
    let policy_ref = match policy_ref {
        Some(policy_ref) => policy_ref,
        None => {
            let policy = &eval.host_context.authority.policy;
            if policy.active_trace_specs.is_empty() && policy.trace_spec_facts.is_empty() {
                return Ok(());
            }
            HostValue::String(etas_host::TRACE_SPEC_RUNTIME_REF.to_owned())
        }
    };
    let id = eval.next_host_request_id();
    let trace = eval.host_trace();
    subject.attributes.push((
        "trace_id".into(),
        HostValue::String(format!("{:?}", trace.trace_id)),
    ));
    let request = PolicyEvaluationRequest {
        id,
        policy_ref,
        subject,
        authority: eval.host_authority(),
        trace,
    };
    let response = HostDispatch::execute(
        eval,
        id,
        HostRequestKind::Policy,
        request.trace_payload(),
        request.authority.clone(),
        request.trace.clone(),
        |operation| host.policy(operation, request),
    )
    .await?;
    match response.decision {
        PolicyDecision::Allow => Ok(()),
        PolicyDecision::Deny { reason } => Err(denied(format!("policy denied request: {reason}"))),
        PolicyDecision::RequireApproval { request } => {
            eval.host_budget().check_time()?;
            let response = HostDispatch::execute_approval(
                eval,
                request.clone(),
                eval.host_authority(),
                |operation| host.approval(operation, request),
            )
            .await?;
            match response.decision {
                etas_host::ApprovalDecision::Approved { grant } => {
                    eval.record_approval_grant(grant);
                    Ok(())
                }
                etas_host::ApprovalDecision::Denied { .. } => {
                    Err(denied("policy approval was denied"))
                }
            }
        }
    }
}

fn denied(message: impl Into<String>) -> HostError {
    HostError::new(HostErrorCode::AuthorityDenied, message)
}

pub(super) fn policy_failure_message(boundary: &str, error: &HostError) -> String {
    if error.code == HostErrorCode::AuthorityDenied {
        format!("{boundary} {}", error.message)
    } else {
        format!("{boundary} policy boundary failed: {}", error.message)
    }
}

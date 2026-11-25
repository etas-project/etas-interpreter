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

pub(in crate::driver) enum AuthorizationRequest {
    Policy(PolicyEvaluationRequest),
    Approval {
        request: etas_host::ApprovalRequest,
        authority: etas_host::AuthorityContext,
    },
}

impl crate::control::PolicyAuthorization {
    pub(in crate::driver) fn new(
        eval: &EvalContext<'_>,
        policy_ref: Option<HostValue>,
        subject: PolicySubject,
    ) -> Self {
        let mut policies = vec![(
            policy_ref,
            eval.task.host_context().authority.policy.clone(),
        )];
        for restriction in &eval.task.authority_constraints() {
            let entry = (
                restriction.policy.boundary_policy_ref.clone(),
                restriction.policy.clone(),
            );
            if !policies.contains(&entry) {
                policies.push(entry);
            }
        }
        if matches!(subject.kind.as_str(), "model" | "tool") {
            for restriction in eval.task.model_restrictions() {
                if restriction.policy_ref.is_some() {
                    let entry = (restriction.policy_ref, restriction.policy);
                    if !policies.contains(&entry) {
                        policies.push(entry);
                    }
                }
            }
        }
        Self {
            policies: policies
                .into_iter()
                .filter_map(|(reference, policy)| {
                    let reference = reference.or_else(|| {
                        (!policy.active_trace_specs.is_empty()
                            || !policy.trace_spec_facts.is_empty())
                        .then(|| HostValue::String(etas_host::TRACE_SPEC_RUNTIME_REF.to_owned()))
                    })?;
                    Some((reference, policy))
                })
                .collect(),
            subject,
            approval: None,
            requested_grants: None,
        }
    }

    pub(in crate::driver) fn next_request(
        &mut self,
        eval: &mut EvalContext<'_>,
    ) -> Result<Option<AuthorizationRequest>, HostError> {
        eval.task.host_execution().signal()?.check()?;
        eval.host_budget().check_time()?;
        if let Some(request) = self.approval.take() {
            self.requested_grants = Some(request.requested_grants.clone());
            return Ok(Some(AuthorizationRequest::Approval {
                request,
                authority: eval.host_authority(),
            }));
        }
        let Some((policy_ref, policy)) = self.policies.pop_front() else {
            return Ok(None);
        };
        let id = eval.next_host_request_id();
        let trace = eval.host_trace();
        let mut subject = self.subject.clone();
        subject.attributes.push((
            "trace_id".into(),
            HostValue::String(format!("{:?}", trace.trace_id)),
        ));
        let mut authority = eval.host_authority();
        authority.policy = policy;
        Ok(Some(AuthorizationRequest::Policy(
            PolicyEvaluationRequest {
                id,
                policy_ref,
                subject,
                authority,
                trace,
            },
        )))
    }

    pub(in crate::driver) fn accept_policy(
        &mut self,
        response: etas_host::PolicyResponse,
    ) -> Result<(), HostError> {
        match response.decision {
            PolicyDecision::Allow => Ok(()),
            PolicyDecision::Deny { reason } => {
                Err(denied(format!("policy denied request: {reason}")))
            }
            PolicyDecision::RequireApproval { request } => {
                self.approval = Some(request);
                Ok(())
            }
        }
    }

    pub(in crate::driver) fn accept_approval(
        &mut self,
        eval: &mut EvalContext<'_>,
        response: etas_host::ApprovalResponse,
    ) -> Result<(), HostError> {
        let requested = self
            .requested_grants
            .take()
            .expect("policy approval request");
        match response.decision {
            etas_host::ApprovalDecision::Approved { grant } => {
                if !requested
                    .iter()
                    .all(|requested| grant.grants.contains(requested))
                {
                    return Err(denied("approval did not grant all requested authority"));
                }
                eval.record_approval_grant(grant)
            }
            etas_host::ApprovalDecision::Denied { .. } => Err(denied("policy approval was denied")),
        }
    }
}

/// The same authorization state is driven inline by existing boundaries and by
/// the reactor for managed admission. Grants and independent policy order agree.
pub(in crate::driver) async fn authorize_before_boundary(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    policy_ref: Option<HostValue>,
    subject: PolicySubject,
) -> Result<(), HostError> {
    let mut authorization = crate::control::PolicyAuthorization::new(eval, policy_ref, subject);
    while let Some(request) = authorization.next_request(eval)? {
        match request {
            AuthorizationRequest::Policy(request) => {
                let response = HostDispatch::execute(
                    eval,
                    request.id,
                    HostRequestKind::Policy,
                    request.trace_payload(),
                    request.authority.clone(),
                    request.trace.clone(),
                    |operation| host.policy(operation, request),
                )
                .await?;
                authorization.accept_policy(response)?;
            }
            AuthorizationRequest::Approval { request, authority } => {
                let response =
                    HostDispatch::execute_approval(eval, request.clone(), authority, |operation| {
                        host.approval(operation, request)
                    })
                    .await?;
                authorization.accept_approval(eval, response)?;
            }
        }
    }
    Ok(())
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

use etas_host::{HostValue, PolicySubject};

use crate::{
    eval::{EvalContext, machine::EvalMachine},
    host::HostServices,
};

use super::policy::{boundary_policy_ref_for, evaluate_before_boundary};

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    pending: &crate::control::PendingModel,
    machine: &mut EvalMachine,
) -> bool {
    let policy_ref = boundary_policy_ref_for(eval, pending.request.policy_ref.clone());
    if !evaluate_before_boundary(
        eval,
        host,
        policy_ref,
        policy_subject(&pending.request),
        pending.span,
        "model",
    )
    .await
    {
        return false;
    }

    let request_id = pending.request.id;
    eval.record_host_request_sent(request_id);
    let result = host.model(pending.request.clone()).await;
    match &result {
        Ok(response) => eval.record_host_response_received(response.id),
        Err(_) => eval.record_host_response_received(request_id),
    }
    machine.resume_model_result(result);
    true
}

fn policy_subject(request: &etas_host::ModelRequest) -> PolicySubject {
    let mut attributes = vec![
        (
            "action_kind".to_owned(),
            HostValue::String("model".to_owned()),
        ),
        (
            "qualified_action".to_owned(),
            HostValue::String("Model.invoke".to_owned()),
        ),
        (
            "model".to_owned(),
            HostValue::String(request.model.0.clone()),
        ),
        (
            "resource".to_owned(),
            HostValue::String(request.model.0.clone()),
        ),
        (
            "tool_count".to_owned(),
            HostValue::UInt(request.tools.len() as u128),
        ),
    ];
    if let Some(provider) = &request.provider {
        attributes.push(("provider".to_owned(), HostValue::String(provider.0.clone())));
    }
    PolicySubject {
        kind: "model".to_owned(),
        attributes,
    }
}

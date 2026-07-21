use etas_effects::HostRequirementKind;

use crate::{
    eval::{
        EvalContext,
        machine::{EvalMachine, PendingTool, PendingToolDispatch},
    },
    host::HostServices,
};

use super::policy::{boundary_policy_ref_for, evaluate_before_boundary};

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    pending: PendingTool,
    machine: &mut EvalMachine,
) -> bool {
    let policy_ref = boundary_policy_ref_for(eval, pending.policy_ref.clone());
    if !evaluate_before_boundary(
        eval,
        host,
        policy_ref,
        pending.policy_subject,
        pending.span,
        "tool",
    )
    .await
    {
        return false;
    }

    match pending.dispatch {
        PendingToolDispatch::Source => {
            machine.resume_source_tool_approved();
            true
        }
        PendingToolDispatch::Host(request) => {
            let request = *request;
            if !host.availability().supports(HostRequirementKind::ToolCall) {
                eval.diagnostics.push(crate::diagnostics::missing_host_handler(
                    pending.span,
                    format!(
                        "model requested host tool `{}` but the host does not provide ToolCall support",
                        request.tool.name
                    ),
                ));
                return false;
            }
            let request_id = request.id;
            eval.record_host_request_sent(request_id);
            let result = host.tool(request).await;
            match &result {
                Ok(response) => eval.record_host_response_received(response.id),
                Err(_) => eval.record_host_response_received(request_id),
            }
            machine.resume_tool_result(result);
            true
        }
    }
}

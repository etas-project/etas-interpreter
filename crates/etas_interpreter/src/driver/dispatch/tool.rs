use etas_effects::HostRequirementKind;
use etas_host::{HostRequestKind, HostTraceRequest};

use crate::{
    eval::{
        EvalContext,
        machine::{EvalMachine, PendingTool, PendingToolDispatch},
    },
    host::HostServices,
};

use super::{
    host_dispatch::HostDispatch,
    policy::{boundary_policy_ref_for, evaluate_before_boundary},
};

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
            if let Err(error) = request.budget.check_time() {
                machine.resume_tool_result(Err(error));
                return true;
            }
            let request_id = request.id;
            let trace_payload = request.trace_payload();
            let authority = request.authority.clone();
            let trace = request.trace.clone();
            let result = HostDispatch::execute(
                eval,
                request_id,
                HostRequestKind::Tool,
                trace_payload,
                authority,
                trace,
                |operation| host.tool(operation, request),
            )
            .await;
            machine.resume_tool_result(result);
            true
        }
    }
}

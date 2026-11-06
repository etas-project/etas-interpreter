use etas_effects::HostRequirementKind;
use etas_host::{HostRequestKind, HostTraceRequest, ToolResponse};

use crate::{
    eval::{
        EvalContext,
        machine::{EvalMachine, PendingTool, PendingToolDispatch},
    },
    host::HostServices,
};

use super::{
    host_dispatch::{HostCompletion, HostDispatch, PendingHostOperation},
    policy::{boundary_policy_ref_for, evaluate_before_boundary},
};

type ToolCall<'host> = crate::host::HostFuture<'host, Result<ToolResponse, etas_host::HostError>>;

pub(in crate::driver) enum ToolDispatchStart<'host> {
    Complete(bool),
    Pending(PendingToolHostDispatch<'host>),
}

pub(in crate::driver) struct PendingToolHostDispatch<'host> {
    pending: PendingHostOperation<ToolResponse, ToolCall<'host>>,
}

pub(in crate::driver) struct CompletedToolHostDispatch {
    completion: HostCompletion<ToolResponse>,
}

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    pending: PendingTool,
    machine: &mut EvalMachine,
) -> bool {
    match begin(eval, host, pending, machine).await {
        ToolDispatchStart::Complete(complete) => complete,
        ToolDispatchStart::Pending(pending) => {
            finish(eval, machine, pending.poll().await);
            true
        }
    }
}

pub(in crate::driver) async fn begin<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    pending: PendingTool,
    machine: &mut EvalMachine,
) -> ToolDispatchStart<'host> {
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
        return ToolDispatchStart::Complete(false);
    }

    match pending.dispatch {
        PendingToolDispatch::Source => {
            machine.resume_source_tool_approved();
            ToolDispatchStart::Complete(true)
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
                return ToolDispatchStart::Complete(false);
            }
            if let Err(error) = request.budget.check_time() {
                machine.resume_tool_result(Err(error));
                return ToolDispatchStart::Complete(true);
            }
            let request_id = request.id;
            let trace_payload = request.trace_payload();
            let authority = request.authority.clone();
            let trace = request.trace.clone();
            match HostDispatch::begin(
                eval,
                request_id,
                HostRequestKind::Tool,
                trace_payload,
                authority,
                trace,
                |operation| host.tool(operation, request),
            ) {
                Ok(pending) => ToolDispatchStart::Pending(PendingToolHostDispatch { pending }),
                Err(error) => {
                    machine.resume_tool_result(Err(error));
                    ToolDispatchStart::Complete(true)
                }
            }
        }
    }
}

pub(in crate::driver) fn finish(
    eval: &mut EvalContext<'_>,
    machine: &mut EvalMachine,
    completed: CompletedToolHostDispatch,
) {
    machine.resume_tool_result(HostDispatch::finish(eval, completed.completion));
}

impl PendingToolHostDispatch<'_> {
    pub(in crate::driver) fn operation_id(&self) -> etas_host::execution::OperationId {
        self.pending.operation_id()
    }

    pub(in crate::driver) async fn poll(self) -> CompletedToolHostDispatch {
        CompletedToolHostDispatch {
            completion: HostDispatch::poll(self.pending).await,
        }
    }
}

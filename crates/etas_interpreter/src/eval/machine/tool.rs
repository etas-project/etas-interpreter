use super::{
    EvalMachine, MachinePoll,
    frame::EvalFrame,
    state::{PendingBoundary, PendingTool, PendingToolDispatch},
};
use crate::{
    control::{Continuation, ControlSignal, ExecutionFault},
    eval::{
        EvalContext,
        host_value::decode_checked_host_value,
        tool::{ResolvedToolInvocation, ToolInvocation, ToolRejection},
    },
};
use etas_core::AnalysisDiagnosticCode;
use etas_host::{HostError, HostValue, PolicySubject, ToolRequest, ToolResponse};

type Transition = Result<ControlSignal, MachinePoll>;

impl EvalMachine {
    pub(super) fn begin_tool_invocation(
        &mut self,
        ctx: &mut EvalContext<'_>,
        invocation: Box<ToolInvocation>,
    ) -> Transition {
        let resolved = match ctx.resolve_tool_invocation(&invocation) {
            Ok(resolved) => resolved,
            Err(error) => return Ok(ctx.tool_invocation_error(&invocation, error)),
        };
        let key = ctx
            .tool_invocation_fingerprint(&invocation, &resolved)
            .map_err(MachinePoll::Fault)?;
        let occurrence = ctx.tool_invocation_occurrence(&invocation, &resolved);
        if let Some(value) = ctx
            .completed_host_boundary_result(&occurrence, "tool", &key, invocation.span)
            .map_err(MachinePoll::Fault)?
        {
            return Ok(ControlSignal::Value(value));
        }
        let dispatch = match &resolved.binding.source {
            Some(source) => {
                if let Err(fault) = ctx.prepare_source_tool_call(
                    source.item,
                    resolved.args.clone(),
                    invocation.span,
                ) {
                    if fault.code == AnalysisDiagnosticCode::InvalidArguments {
                        return Ok(ctx.tool_invocation_rejection(
                            &invocation,
                            ToolRejection {
                                code: "invalid_arguments",
                                message: fault.message,
                            },
                        ));
                    }
                    return Err(MachinePoll::Fault(fault));
                }
                PendingToolDispatch::Source
            }
            None => PendingToolDispatch::Host(Box::new(ToolRequest {
                id: invocation.request,
                tool: resolved.binding.schema.tool.clone(),
                args: resolved.args.clone(),
                authority: ctx.host_authority(),
                trace: ctx.host_trace(),
                budget: ctx.host_budget(),
            })),
        };
        let pending = PendingTool {
            dispatch,
            policy_ref: invocation.context.policy.policy_ref.clone(),
            policy_subject: policy_subject(&resolved),
            span: invocation.span,
        };
        self.push_continuation_frames(Continuation::ToolInvocationResult(invocation));
        Err(MachinePoll::Yield(PendingBoundary::Tool(Box::new(pending))))
    }

    fn pop_tool_invocation(&mut self) -> Box<ToolInvocation> {
        let Some(EvalFrame::Continuation(frame)) = self.pop_frame() else {
            unreachable!("native tool dispatch owns the top machine continuation");
        };
        let Continuation::ToolInvocationResult(invocation) = frame.continuation else {
            unreachable!("native tool dispatch owns the top machine continuation");
        };
        invocation
    }

    pub(super) fn resume_native_tool_approved(&mut self, ctx: &mut EvalContext<'_>) -> Transition {
        let invocation = self.pop_tool_invocation();
        let resolved = match ctx.resolve_tool_invocation(&invocation) {
            Ok(resolved) => resolved,
            Err(error) => return Ok(ctx.tool_invocation_error(&invocation, error)),
        };
        let source = resolved.binding.source.ok_or_else(|| {
            MachinePoll::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                invocation.span,
                "source approval refers to a host tool",
            ))
        })?;
        let (target, args) = ctx
            .prepare_source_tool_call(source.item, resolved.args, invocation.span)
            .map_err(MachinePoll::Fault)?;
        let span = invocation.span;
        self.push_continuation_frames(Continuation::ToolInvocationResult(invocation));
        Ok(ctx.execute_call_target(target, args, span))
    }

    pub(super) fn resume_native_tool_result(
        &mut self,
        ctx: &mut EvalContext<'_>,
        response: Result<ToolResponse, HostError>,
    ) -> Transition {
        let invocation = self.pop_tool_invocation();
        let fault = |message| {
            MachinePoll::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                invocation.span,
                message,
            ))
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let message = format!(
                    "native tool host boundary failed: {}: {}",
                    error.code.as_str(),
                    error.message
                );
                if let Some(signal) = ctx.cancellation_signal(invocation.span) {
                    return Ok(signal);
                }
                return match self.retry_boundary_failure(
                    ctx,
                    Continuation::BlockValue,
                    invocation.span,
                    message.clone(),
                ) {
                    Some(signal) => Ok(signal),
                    None => Err(fault(message)),
                };
            }
        };
        let value = response
            .result
            .map_err(|error| fault(format!("native tool failed: {}", error.message)))?;
        let resolved = match ctx.resolve_tool_invocation(&invocation) {
            Ok(resolved) => resolved,
            Err(error) => return Ok(ctx.tool_invocation_error(&invocation, error)),
        };
        let value = decode_checked_host_value(value, resolved.binding.output, ctx.checked)
            .map_err(fault)?;
        Ok(ctx.complete_tool_invocation(*invocation, value))
    }
}

fn policy_subject(tool: &ResolvedToolInvocation) -> PolicySubject {
    PolicySubject {
        kind: "tool".into(),
        attributes: vec![
            ("action_kind".into(), HostValue::String("tool".into())),
            (
                "qualified_action".into(),
                HostValue::String("Tool.call".into()),
            ),
            (
                "tool".into(),
                HostValue::String(tool.binding.schema.tool.name.clone()),
            ),
            (
                "qualified_tool".into(),
                HostValue::String(
                    tool.binding
                        .schema
                        .tool
                        .qualified_name
                        .clone()
                        .unwrap_or_else(|| tool.identity.clone()),
                ),
            ),
            ("revision".into(), HostValue::String(tool.revision.clone())),
            ("resource".into(), HostValue::String(tool.identity.clone())),
        ],
    }
}

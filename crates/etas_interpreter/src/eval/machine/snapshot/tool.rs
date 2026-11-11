use super::{CaptureContext, RestoreContext};
use crate::{
    control::{AgentOperationKind, AgentOperationOrigin, CallTarget, CheckedAgentOperation},
    eval::tool::ToolInvocation,
    orchestration::{ToolInvocationOriginSnapshot as Origin, ToolInvocationSnapshot},
};

impl ToolInvocationSnapshot {
    pub(super) fn capture(
        invocation: &ToolInvocation,
        context: &mut CaptureContext,
    ) -> Result<Box<Self>, String> {
        Ok(Box::new(Self {
            call: match &invocation.call.origin {
                AgentOperationOrigin::Standard(call) => {
                    Origin::Standard(context.call_target(&CallTarget::StdIntrinsic(call.clone()))?)
                }
                AgentOperationOrigin::Controller {
                    inference,
                    operation: AgentOperationKind::Invoke,
                } => Origin::Controller {
                    inference: *inference,
                },
                _ => return Err("tool invocation has a non-tool operation origin".into()),
            },
            input: context.value(&invocation.input)?,
            context: Box::new(super::capture_agent_context(
                &invocation.context,
                &mut context.revisions,
            )),
            request: invocation.request,
            span: invocation.span,
        }))
    }

    pub(super) fn restore(
        self,
        context: &mut RestoreContext,
    ) -> Result<Box<ToolInvocation>, String> {
        let request_context = context.agent_context(&self.context)?;
        let call = match self.call {
            Origin::Standard(call) => {
                let CallTarget::StdIntrinsic(call) = context.restore_call_target(call)? else {
                    return Err("native tool invocation requires its checked std intrinsic".into());
                };
                CheckedAgentOperation::standard(&call)
            }
            Origin::Controller { inference } => CheckedAgentOperation::controller(
                &request_context.program.checked,
                inference,
                AgentOperationKind::Invoke,
            )?,
        };
        Ok(Box::new(ToolInvocation {
            call,
            input: self.input.restore_with(context)?,
            context: request_context,
            request: self.request,
            span: self.span,
        }))
    }
}

impl crate::orchestration::AgentControllerSnapshot {
    pub(super) fn capture(
        state: &crate::eval::agent::controller::AgentController,
        context: &mut CaptureContext,
    ) -> Result<Box<Self>, String> {
        Ok(Box::new(Self {
            inference: state.inference,
            conversation: context.value(&crate::value::InterpValue::Conversation(
                state.conversation.clone(),
            ))?,
            calls: context.value(&crate::value::InterpValue::Array(state.calls.clone()))?,
            next_call: state.next_call,
            results: state
                .results
                .iter()
                .map(|value| context.value(value))
                .collect::<Result<_, _>>()?,
            round: state.round,
            repairs: state.repairs,
            max_tool_rounds: state.max_tool_rounds,
            span: state.span,
        }))
    }

    pub(super) fn restore(
        self,
        context: &mut RestoreContext,
    ) -> Result<Box<crate::eval::agent::controller::AgentController>, String> {
        let crate::value::InterpValue::Conversation(conversation) =
            self.conversation.restore_with(context)?
        else {
            return Err("agent controller has no Conversation".into());
        };
        let crate::value::InterpValue::Array(calls) = self.calls.restore_with(context)? else {
            return Err("agent controller has no tool-call Array".into());
        };
        Ok(Box::new(crate::eval::agent::controller::AgentController {
            inference: self.inference,
            conversation,
            calls,
            next_call: self.next_call,
            results: self
                .results
                .into_iter()
                .map(|value| value.restore_with(context))
                .collect::<Result<_, _>>()?,
            round: self.round,
            repairs: self.repairs,
            max_tool_rounds: self.max_tool_rounds,
            span: self.span,
        }))
    }
}

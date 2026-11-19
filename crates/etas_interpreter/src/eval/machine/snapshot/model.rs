use super::{RestoreContext, capture_context::CaptureContext};
use crate::api::{ModelExecutionPolicy, ModelResponseDecodePolicy};
use crate::control::{ModelDecode, PendingModel};
use crate::eval::machine::frame::ModelRequestFrame;
use crate::orchestration::{
    ModelDecodeSnapshot, ModelExecutionPolicySnapshot, ModelRequestFrameSnapshot,
    ModelResponseDecodeSnapshot, PendingModelSnapshot,
};

pub(crate) fn capture_model_policy(policy: &ModelExecutionPolicy) -> ModelExecutionPolicySnapshot {
    ModelExecutionPolicySnapshot {
        provider: policy.provider.clone(),
        provider_capabilities: policy.provider_capabilities,
        model: policy.model.clone(),
        model_locked: policy.model_locked,
        tools: policy.tools.clone(),
        tool_choice: policy.tool_choice.clone(),
        policy_ref: policy.policy_ref.clone(),
        options: policy.options.clone(),
        budget: policy.budget.clone(),
        response_decode: match policy.response_decode {
            ModelResponseDecodePolicy::String => ModelResponseDecodeSnapshot::String,
            ModelResponseDecodePolicy::ModelResponse => ModelResponseDecodeSnapshot::ModelResponse,
        },
        max_tool_rounds: policy.max_tool_rounds,
    }
}

pub(crate) fn restore_model_policy(policy: ModelExecutionPolicySnapshot) -> ModelExecutionPolicy {
    ModelExecutionPolicy {
        provider: policy.provider,
        provider_capabilities: policy.provider_capabilities,
        model: policy.model,
        model_locked: policy.model_locked,
        tools: policy.tools,
        tool_choice: policy.tool_choice,
        policy_ref: policy.policy_ref,
        options: policy.options,
        budget: policy.budget,
        response_decode: match policy.response_decode {
            ModelResponseDecodeSnapshot::String => ModelResponseDecodePolicy::String,
            ModelResponseDecodeSnapshot::ModelResponse => ModelResponseDecodePolicy::ModelResponse,
        },
        max_tool_rounds: policy.max_tool_rounds,
    }
}

impl ModelRequestFrameSnapshot {
    pub(in crate::eval::machine::snapshot) fn capture(
        frame: &ModelRequestFrame,
        context: &mut CaptureContext,
    ) -> Result<Self, String> {
        Ok(Self {
            boundary_request_id: frame.boundary_request_id,
            pending: PendingModelSnapshot::capture(&frame.pending, context)?,
            boundary_key: frame.boundary_key,
            outer_continuation: context.continuation(&frame.outer_continuation)?,
        })
    }

    pub(crate) fn restore(
        self,
        current: &crate::api::HostExecutionContext,
        context: &mut RestoreContext,
    ) -> Result<ModelRequestFrame, String> {
        Ok(ModelRequestFrame {
            boundary_request_id: self.boundary_request_id,
            pending: self.pending.restore(current, context)?,
            boundary_key: self.boundary_key,
            outer_continuation: self.outer_continuation.restore_with(context)?,
        })
    }
}

impl PendingModelSnapshot {
    fn capture(pending: &PendingModel, context: &mut CaptureContext) -> Result<Self, String> {
        Ok(Self {
            request: crate::orchestration::ModelRequestSnapshot::capture(&pending.request),
            decode: match pending.decode {
                ModelDecode::String => ModelDecodeSnapshot::String,
                ModelDecode::ModelResponse => ModelDecodeSnapshot::ModelResponse,
                ModelDecode::Typed(ty) => ModelDecodeSnapshot::Typed(ty),
                ModelDecode::Step { result, output } => {
                    ModelDecodeSnapshot::Step { result, output }
                }
            },
            span: pending.span,
            continuation: context.continuation(&pending.continuation)?,
        })
    }

    fn restore(
        self,
        current: &crate::api::HostExecutionContext,
        context: &mut RestoreContext,
    ) -> Result<PendingModel, String> {
        Ok(PendingModel {
            request: self
                .request
                .restore(current, context.locked_model.as_ref())
                .map_err(|error| error.to_string())?,
            decode: match self.decode {
                ModelDecodeSnapshot::String => ModelDecode::String,
                ModelDecodeSnapshot::ModelResponse => ModelDecode::ModelResponse,
                ModelDecodeSnapshot::Typed(ty) => ModelDecode::Typed(ty),
                ModelDecodeSnapshot::Step { result, output } => {
                    ModelDecode::Step { result, output }
                }
            },
            span: self.span,
            continuation: self.continuation.restore_with(context)?,
        })
    }
}

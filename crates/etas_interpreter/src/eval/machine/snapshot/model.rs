use crate::control::{ModelDecode, PendingModel, SourceToolBinding};
use crate::eval::machine::frame::{
    HostToolProgress, ModelLoopFrame, ModelRepairState, SourceToolReturnFrame,
};
use crate::orchestration::{
    ContinuationSnapshot, HostToolProgressSnapshot, ModelDecodeSnapshot, ModelLoopFrameSnapshot,
    ModelRepairSnapshot, PendingModelSnapshot, SourceToolBindingSnapshot,
    SourceToolReturnFrameSnapshot,
};

impl ModelLoopFrameSnapshot {
    pub(crate) fn capture(frame: &ModelLoopFrame) -> Result<Self, String> {
        Ok(Self {
            pending: PendingModelSnapshot::capture(&frame.pending)?,
            round: frame.round,
            repair: ModelRepairSnapshot {
                attempts: frame.repair.attempts,
                last_kind: frame.repair.last_kind.clone(),
            },
            last_tool_error: frame.last_tool_error.clone(),
            remaining_tool_calls: frame.remaining_tool_calls.clone(),
            completed_tool_result: frame.completed_tool_result,
            current_host_tool: frame.current_host_tool.as_ref().map(|progress| {
                HostToolProgressSnapshot {
                    call: progress.call.clone(),
                    boundary_key: progress.boundary_key.clone(),
                }
            }),
            boundary_key: frame.boundary_key.clone(),
            outer_continuation: ContinuationSnapshot::capture(&frame.outer_continuation)?,
        })
    }

    pub(crate) fn restore(self) -> Result<ModelLoopFrame, String> {
        Ok(ModelLoopFrame {
            pending: self.pending.restore()?,
            round: self.round,
            repair: ModelRepairState {
                attempts: self.repair.attempts,
                last_kind: self.repair.last_kind,
            },
            last_tool_error: self.last_tool_error,
            remaining_tool_calls: self.remaining_tool_calls,
            completed_tool_result: self.completed_tool_result,
            current_host_tool: self.current_host_tool.map(|progress| HostToolProgress {
                call: progress.call,
                boundary_key: progress.boundary_key,
            }),
            boundary_key: self.boundary_key,
            outer_continuation: self.outer_continuation.restore()?,
        })
    }
}

impl PendingModelSnapshot {
    fn capture(pending: &PendingModel) -> Result<Self, String> {
        Ok(Self {
            request: pending.request.clone(),
            decode: match pending.decode {
                ModelDecode::String => ModelDecodeSnapshot::String,
                ModelDecode::ModelResponse => ModelDecodeSnapshot::ModelResponse,
                ModelDecode::Typed(ty) => ModelDecodeSnapshot::Typed(ty),
            },
            max_tool_rounds: pending.max_tool_rounds,
            source_tools: pending
                .source_tools
                .iter()
                .map(SourceToolBindingSnapshot::capture)
                .collect(),
            span: pending.span,
            continuation: ContinuationSnapshot::capture(&pending.continuation)?,
        })
    }

    fn restore(self) -> Result<PendingModel, String> {
        Ok(PendingModel {
            request: self.request,
            decode: match self.decode {
                ModelDecodeSnapshot::String => ModelDecode::String,
                ModelDecodeSnapshot::ModelResponse => ModelDecode::ModelResponse,
                ModelDecodeSnapshot::Typed(ty) => ModelDecode::Typed(ty),
            },
            max_tool_rounds: self.max_tool_rounds,
            source_tools: self
                .source_tools
                .into_iter()
                .map(SourceToolBindingSnapshot::restore)
                .collect(),
            span: self.span,
            continuation: self.continuation.restore()?,
        })
    }
}

impl SourceToolBindingSnapshot {
    fn capture(binding: &SourceToolBinding) -> Self {
        Self {
            name: binding.name.clone(),
            qualified_name: binding.qualified_name.clone(),
            item: binding.item,
        }
    }

    fn restore(self) -> SourceToolBinding {
        SourceToolBinding {
            name: self.name,
            qualified_name: self.qualified_name,
            item: self.item,
        }
    }
}

impl SourceToolReturnFrameSnapshot {
    pub(crate) fn capture(frame: &SourceToolReturnFrame) -> Result<Self, String> {
        Ok(Self {
            tool_call_id: frame.tool_call_id.clone(),
            tool_name: frame.tool_name.clone(),
            binding: SourceToolBindingSnapshot::capture(&frame.binding),
            args: frame.args.clone(),
            boundary_key: frame.boundary_key.clone(),
            output_schema: frame.output_schema.clone(),
            model_loop: Box::new(ModelLoopFrameSnapshot::capture(&frame.model_loop)?),
        })
    }

    pub(crate) fn restore(self) -> Result<SourceToolReturnFrame, String> {
        Ok(SourceToolReturnFrame {
            tool_call_id: self.tool_call_id,
            tool_name: self.tool_name,
            binding: self.binding.restore(),
            args: self.args,
            boundary_key: self.boundary_key,
            output_schema: self.output_schema,
            model_loop: Box::new(self.model_loop.restore()?),
        })
    }
}

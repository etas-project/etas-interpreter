use crate::{
    eval::machine::frame::EvalFrame,
    orchestration::{
        ContinuationSnapshot, MachineFrameSnapshot, ModelLoopFrameSnapshot,
        SourceToolReturnFrameSnapshot,
    },
};

use super::capture_context::CaptureContext;

impl MachineFrameSnapshot {
    pub(in crate::eval::machine) fn capture(
        frame: &EvalFrame,
        context: &mut CaptureContext,
    ) -> Result<Self, String> {
        Ok(match frame {
            EvalFrame::Block(frame) => Self::Block {
                continuation: context.continuation(&frame.continuation)?,
            },
            EvalFrame::Expr(frame) => Self::Expr {
                continuation: context.continuation(&frame.continuation)?,
            },
            EvalFrame::Call(frame) => Self::Call {
                continuation: context.continuation(&frame.continuation)?,
                span: frame.span,
            },
            EvalFrame::Continuation(frame) => Self::Continuation {
                continuation: context.continuation(&frame.continuation)?,
            },
            EvalFrame::Handler(frame) => Self::Handler {
                continuation: ContinuationSnapshot::HandleBoundary {
                    scope_id: frame.scope_id,
                    inner: context.continuation(&frame.inner)?.into(),
                    handlers: frame.handlers.clone(),
                    span: frame.span,
                    frame: context.frame(&frame.frame)?,
                },
            },
            EvalFrame::Retry(frame) => Self::Retry {
                continuation: ContinuationSnapshot::RetryAttempt {
                    retry: frame.retry.clone(),
                    body: frame.body,
                    attempts: frame.attempts,
                    next_attempt: frame.next_attempt,
                    block: frame.block,
                    next_stmt_index: frame.next_stmt_index,
                    frame: context.frame(&frame.frame)?,
                },
            },
            EvalFrame::ModelLoop(frame) => {
                Self::ModelLoop(Box::new(ModelLoopFrameSnapshot::capture(frame, context)?))
            }
            EvalFrame::SourceToolReturn(frame) => {
                Self::SourceToolReturn(SourceToolReturnFrameSnapshot::capture(frame, context)?)
            }
        })
    }
}

#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod tests;

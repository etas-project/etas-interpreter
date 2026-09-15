use crate::{
    eval::machine::frame::EvalFrame,
    orchestration::{
        ContinuationSnapshot, MachineFrameSnapshot, ModelLoopFrameSnapshot,
        SourceToolReturnFrameSnapshot,
    },
};

impl MachineFrameSnapshot {
    pub(crate) fn capture(frame: &EvalFrame) -> Result<Self, String> {
        Ok(match frame {
            EvalFrame::Block(frame) => Self::Block {
                continuation: ContinuationSnapshot::capture(&frame.continuation)?,
            },
            EvalFrame::Expr(frame) => Self::Expr {
                continuation: ContinuationSnapshot::capture(&frame.continuation)?,
            },
            EvalFrame::Call(frame) => Self::Call {
                continuation: ContinuationSnapshot::capture(&frame.continuation)?,
                span: frame.span,
            },
            EvalFrame::Continuation(frame) => Self::Continuation {
                continuation: ContinuationSnapshot::capture(&frame.continuation)?,
            },
            EvalFrame::Handler(frame) => Self::Handler {
                continuation: ContinuationSnapshot::HandleBoundary {
                    scope_id: frame.scope_id,
                    inner: ContinuationSnapshot::capture(&frame.inner)?.into(),
                    handlers: frame.handlers.clone(),
                    span: frame.span,
                    frame: super::frame::capture_frame(&frame.frame)?,
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
                    frame: super::frame::capture_frame(&frame.frame)?,
                },
            },
            EvalFrame::ModelLoop(frame) => {
                Self::ModelLoop(Box::new(ModelLoopFrameSnapshot::capture(frame)?))
            }
            EvalFrame::SourceToolReturn(frame) => {
                Self::SourceToolReturn(SourceToolReturnFrameSnapshot::capture(frame)?)
            }
        })
    }
}

#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod tests;

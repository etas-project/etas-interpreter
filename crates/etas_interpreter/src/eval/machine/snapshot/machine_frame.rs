use crate::{
    eval::machine::frame::EvalFrame,
    orchestration::{ContinuationSnapshot, MachineFrameSnapshot, ModelRequestFrameSnapshot},
};

use super::capture_context::CaptureContext;

impl MachineFrameSnapshot {
    pub(in crate::eval::machine) fn restore_local(
        self,
        current: &crate::api::HostExecutionContext,
        context: &mut super::RestoreContext,
    ) -> Result<EvalFrame, String> {
        context.current_host = Some(current.clone());
        use crate::eval::machine::frame::*;
        Ok(match self {
            Self::ProgramReturn(_) => {
                return Err("program boundary was not consumed by scoped restore".into());
            }
            Self::ResidualScope { scope } => {
                EvalFrame::ResidualScope(context.residual_scope(scope)?)
            }
            Self::Block { continuation } => EvalFrame::Block(BlockFrame {
                continuation: continuation.restore_with(context)?,
            }),
            Self::Expr { continuation } => EvalFrame::Expr(ExprFrame {
                continuation: continuation.restore_with(context)?,
            }),
            Self::Call {
                caller,
                continuation,
                span,
            } => EvalFrame::Call(CallFrame {
                caller,
                continuation: continuation.restore_with(context)?,
                span,
            }),
            Self::Continuation { continuation } => EvalFrame::Continuation(ContinuationFrame {
                continuation: continuation.restore_with(context)?,
            }),
            Self::Handler { continuation } => {
                let frame = EvalFrame::from_continuation(continuation.restore_with(context)?);
                if !matches!(frame, EvalFrame::Handler(_)) {
                    return Err(
                        "checkpoint handler frame does not contain a handler boundary".into(),
                    );
                }
                frame
            }
            Self::Retry { continuation } => {
                let frame = EvalFrame::from_continuation(continuation.restore_with(context)?);
                if !matches!(frame, EvalFrame::Retry(_)) {
                    return Err("checkpoint retry frame does not contain a retry attempt".into());
                }
                frame
            }
            Self::ModelRequest(frame) => {
                EvalFrame::ModelRequest(Box::new(frame.restore(current, context)?))
            }
        })
    }

    pub(in crate::eval::machine) fn capture(
        frame: &EvalFrame,
        context: &mut CaptureContext,
    ) -> Result<Self, String> {
        Ok(match frame {
            EvalFrame::PureBoundary(_) => {
                return Err("pure invocation scope cannot cross a checkpoint boundary".into());
            }
            EvalFrame::ProgramReturn(_) => {
                return Err(
                    "compiled revision checkpoint requires a checked artifact resolver".into(),
                );
            }
            EvalFrame::Block(frame) => Self::Block {
                continuation: context.continuation(&frame.continuation)?,
            },
            EvalFrame::Expr(frame) => Self::Expr {
                continuation: context.continuation(&frame.continuation)?,
            },
            EvalFrame::Call(frame) => Self::Call {
                caller: frame.caller,
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
            EvalFrame::ModelRequest(frame) => Self::ModelRequest(Box::new(
                ModelRequestFrameSnapshot::capture(frame, context)?,
            )),
            EvalFrame::ResidualScope(scope) => Self::ResidualScope {
                scope: context.residual_scope(scope)?,
            },
        })
    }
}

#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod tests;

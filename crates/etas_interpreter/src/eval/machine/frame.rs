use etas_core::Span;
use etas_hir::HirBlockId;

use crate::{
    control::{Continuation, ContinuationLink, Frame, PendingModel},
    orchestration::{ActiveHandlerArmRecord, HandlerScopeId, RetryAttemptRecord},
};

#[derive(Clone, Debug)]
pub(crate) struct BlockFrame {
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub(crate) struct CallFrame {
    pub caller: Option<crate::control::CallOrigin>,
    pub continuation: Continuation,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub(crate) struct ExprFrame {
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub(crate) struct ContinuationFrame {
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub(crate) struct HandlerFrame {
    pub scope_id: HandlerScopeId,
    pub inner: ContinuationLink,
    pub handlers: Vec<ActiveHandlerArmRecord>,
    pub span: Span,
    pub frame: Frame,
}

#[derive(Clone, Debug)]
pub(crate) struct RetryFrame {
    pub retry: RetryAttemptRecord,
    pub body: HirBlockId,
    pub attempts: usize,
    pub next_attempt: usize,
    pub block: HirBlockId,
    pub next_stmt_index: usize,
    pub frame: Frame,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelRequestFrame {
    pub boundary_request_id: etas_host::HostRequestId,
    pub pending: PendingModel,
    pub boundary_key: etas_host::replay::ReplayFingerprint,
    pub outer_continuation: Continuation,
}

#[derive(Debug)]
pub(crate) enum EvalFrame {
    PureBoundary(Span),
    ProgramReturn(Box<ProgramReturnFrame>),
    Block(BlockFrame),
    Call(CallFrame),
    Continuation(ContinuationFrame),
    Expr(ExprFrame),
    Handler(HandlerFrame),
    ModelRequest(Box<ModelRequestFrame>),
    Retry(RetryFrame),
    /// The residual scope of one callable invocation. Leaving it on any path
    /// visits the scope-exit protocol; an effect request handled outside it
    /// captures it into the request's continuation instead.
    ResidualScope(crate::control::residual::SharedResidualScope),
}

#[derive(Debug)]
pub(crate) struct ProgramReturnFrame {
    pub site: Option<crate::control::CallSite>,
    pub parent: Option<super::state::ProgramScope>,
    pub parent_program: crate::value::CheckedExecutionProgram,
    pub parent_model_policy: crate::api::ModelExecutionPolicy,
    pub parent_agent_context: Option<Box<crate::control::AgentRequestContext>>,
    pub call: super::scoped_call::ProgramCall,
    pub outer: Continuation,
    pub span: Span,
    pub retry_depth: usize,
}

impl EvalFrame {
    pub(crate) fn from_continuation(continuation: Continuation) -> Self {
        match continuation {
            Continuation::CallBoundary {
                caller,
                span,
                outer,
            } => Self::Call(CallFrame {
                caller,
                continuation: outer.into_value(),
                span,
            }),
            Continuation::HandleBoundary {
                scope_id,
                inner,
                handlers,
                span,
                frame,
            } => Self::Handler(HandlerFrame {
                scope_id,
                inner,
                handlers,
                span,
                frame,
            }),
            Continuation::RetryAttempt {
                retry,
                body,
                attempts,
                next_attempt,
                block,
                next_stmt_index,
                frame,
            } => Self::Retry(RetryFrame {
                retry,
                body,
                attempts,
                next_attempt,
                block,
                next_stmt_index,
                frame,
            }),
            Continuation::ResidualScope(scope) => Self::ResidualScope(scope),
            continuation => Self::Continuation(ContinuationFrame { continuation }),
        }
    }

    pub(crate) fn into_continuation(self) -> Result<Continuation, String> {
        Ok(match self {
            Self::PureBoundary(_) => {
                return Err("pure invocation scope cannot be suspended as a continuation".into());
            }
            Self::ProgramReturn(_) => {
                return Err(
                    "revision boundary cannot be flattened into a program-local continuation"
                        .into(),
                );
            }
            Self::Block(frame) => frame.continuation,
            Self::Call(frame) => frame.continuation,
            Self::Continuation(frame) => frame.continuation,
            Self::Expr(frame) => frame.continuation,
            Self::Handler(frame) => Continuation::HandleBoundary {
                scope_id: frame.scope_id,
                inner: frame.inner,
                handlers: frame.handlers,
                span: frame.span,
                frame: frame.frame,
            },
            Self::Retry(frame) => Continuation::RetryAttempt {
                retry: frame.retry,
                body: frame.body,
                attempts: frame.attempts,
                next_attempt: frame.next_attempt,
                block: frame.block,
                next_stmt_index: frame.next_stmt_index,
                frame: frame.frame,
            },
            Self::ModelRequest(frame) => frame.outer_continuation,
            Self::ResidualScope(scope) => Continuation::ResidualScope(scope),
        })
    }

    pub(crate) fn retry_continuation(&self) -> Option<Continuation> {
        let continuation = match self {
            Self::PureBoundary(_) => return None,
            Self::ProgramReturn(_) => return None,
            Self::Block(frame) => &frame.continuation,
            Self::Call(frame) => &frame.continuation,
            Self::Continuation(frame) => &frame.continuation,
            Self::Expr(frame) => &frame.continuation,
            Self::Handler(_) | Self::ResidualScope(_) => return None,
            Self::Retry(frame) => {
                return Some(Continuation::RetryAttempt {
                    retry: frame.retry.clone(),
                    body: frame.body,
                    attempts: frame.attempts,
                    next_attempt: frame.next_attempt,
                    block: frame.block,
                    next_stmt_index: frame.next_stmt_index,
                    frame: frame.frame.clone(),
                });
            }
            Self::ModelRequest(frame) => &frame.outer_continuation,
        };
        continuation.find_retry_attempt().cloned()
    }
}

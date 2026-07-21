use etas_core::Span;
use etas_hir::HirBlockId;

use crate::{
    control::{Continuation, Frame, PendingModel, SourceToolBinding},
    orchestration::{ActiveHandlerArmRecord, HandlerScopeId, RetryAttemptRecord},
};
use etas_host::{HostSchema, HostValue, ModelToolCall};

#[derive(Clone, Debug)]
pub(crate) struct BlockFrame {
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub(crate) struct CallFrame {
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
    pub inner: Box<Continuation>,
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

#[derive(Clone, Debug, Default)]
pub(crate) struct ModelRepairState {
    pub attempts: usize,
    pub last_kind: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct HostToolProgress {
    pub call: ModelToolCall,
    pub boundary_key: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelLoopFrame {
    pub pending: PendingModel,
    pub round: usize,
    pub repair: ModelRepairState,
    pub last_tool_error: Option<String>,
    pub remaining_tool_calls: Vec<ModelToolCall>,
    pub completed_tool_result: bool,
    pub current_host_tool: Option<HostToolProgress>,
    pub boundary_key: String,
    pub outer_continuation: Continuation,
}

#[derive(Clone, Debug)]
pub(crate) struct SourceToolReturnFrame {
    pub tool_call_id: String,
    pub tool_name: String,
    pub binding: SourceToolBinding,
    pub args: HostValue,
    pub boundary_key: String,
    pub output_schema: Option<HostSchema>,
    pub model_loop: Box<ModelLoopFrame>,
}

#[derive(Clone, Debug)]
pub(crate) enum EvalFrame {
    Block(BlockFrame),
    Call(CallFrame),
    Continuation(ContinuationFrame),
    Expr(ExprFrame),
    Handler(HandlerFrame),
    ModelLoop(Box<ModelLoopFrame>),
    Retry(RetryFrame),
    SourceToolReturn(SourceToolReturnFrame),
}

impl EvalFrame {
    pub(crate) fn from_continuation(continuation: Continuation) -> Self {
        match continuation {
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
            continuation => Self::Continuation(ContinuationFrame { continuation }),
        }
    }

    pub(crate) fn into_continuation(self) -> Continuation {
        match self {
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
            Self::ModelLoop(frame) => frame.outer_continuation,
            Self::SourceToolReturn(frame) => frame.model_loop.outer_continuation,
        }
    }

    pub(crate) fn continuation_clone(&self) -> Continuation {
        self.clone().into_continuation()
    }
}

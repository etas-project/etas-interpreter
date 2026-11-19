use super::{
    CallSiteSnapshot, CallTargetSnapshot, ContinuationSnapshot, ExecutionProgramSnapshot,
    LocalsSnapshot, ValueSnapshot,
};
use etas_core::Span;
use etas_hir::{HirBlockId, HirExprId, HirItemId};
use etas_types::TypeId;

/// The saved input is separate from the stack: a ready task has an input while
/// an await/park waiter retains its continuation in the scheduler.
#[derive(Clone, Debug)]
pub(crate) enum MachineInputSnapshot {
    EnterProgram {
        owner: ExecutionProgramSnapshot,
        call: super::ProgramCallSnapshot,
        args: Vec<ValueSnapshot>,
        span: Span,
    },
    Signal(Box<ControlSignalSnapshot>),
    ModelResult(Result<etas_host::ModelResponse, etas_host::HostError>),
    ToolResult(Result<etas_host::ToolResponse, etas_host::HostError>),
    SourceToolApproved,
}

#[derive(Clone, Debug)]
pub(crate) enum ControlSignalSnapshot {
    HandlerRequest {
        action: etas_hir::ResolvedActionRef,
        inference: Option<Box<super::AgentRequestContextSnapshot>>,
        args: Vec<ValueSnapshot>,
        span: Span,
        continuation: ContinuationSnapshot,
        residual: Option<u32>,
    },
    Error {
        error_type: etas_types::TypeId,
        payload: ValueSnapshot,
        span: Span,
        continuation: ContinuationSnapshot,
        residual: Option<u32>,
    },
    Input(ContinuationInputSnapshot),
    Apply {
        continuation: ContinuationSnapshot,
        input: ContinuationInputSnapshot,
    },
    Checkpoint {
        label: Option<String>,
        continuation: ContinuationSnapshot,
    },
    Expr {
        expr: HirExprId,
        frame: LocalsSnapshot,
        continuation: ContinuationSnapshot,
    },
    ProgramExpr {
        call: Box<super::ExpressionCallSnapshot>,
        args: Vec<ValueSnapshot>,
        span: Span,
        continuation: ContinuationSnapshot,
    },
    Block {
        agent_context: Option<Box<super::checkpoint::AgentRequestContextSnapshot>>,
        block: HirBlockId,
        next_stmt_index: usize,
        frame: LocalsSnapshot,
        continuation: ContinuationSnapshot,
        residual: Option<ResidualActivationSnapshot>,
    },
    Call {
        pure: bool,
        site: Option<CallSiteSnapshot>,
        target: CallTargetSnapshot,
        args: Vec<ValueSnapshot>,
        span: Span,
        continuation: ContinuationSnapshot,
    },
    Task(TaskControlSnapshot),
    Fault(crate::control::ExecutionFault),
}

#[derive(Clone, Debug)]
pub(crate) enum ContinuationInputSnapshot {
    Value(ValueSnapshot),
    Return(ValueSnapshot),
    Resume(ValueSnapshot),
    Finish(ValueSnapshot),
    Break,
    Continue,
    RetryFailure { span: Span, message: String },
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidualActivationSnapshot {
    pub item: HirItemId,
    pub application: usize,
}

#[derive(Clone, Debug)]
pub(crate) enum TaskControlSnapshot {
    NextCompletion {
        scope: u64,
        result_type: TypeId,
        span: Span,
    },
    ResumeHandled {
        owner: u64,
        task: u64,
        handler: super::HandlerScopeId,
        value: ValueSnapshot,
        span: Span,
    },
    Run {
        scope: u64,
        span: Span,
    },
    Requeue {
        token: crate::value::ContinuationTokenValue,
        value: ValueSnapshot,
        span: Span,
    },
    Park {
        owner: u64,
        task: u64,
        token: crate::value::ContinuationTokenValue,
        span: Span,
    },
}

impl MachineInputSnapshot {
    pub(crate) fn signal(&self) -> Option<&ControlSignalSnapshot> {
        match self {
            Self::Signal(signal) => Some(signal),
            _ => None,
        }
    }

    pub(crate) fn continuation_root(&self) -> Option<&ContinuationSnapshot> {
        match self.signal()? {
            ControlSignalSnapshot::Apply { continuation, .. }
            | ControlSignalSnapshot::Error { continuation, .. }
            | ControlSignalSnapshot::HandlerRequest { continuation, .. }
            | ControlSignalSnapshot::Checkpoint { continuation, .. }
            | ControlSignalSnapshot::Expr { continuation, .. }
            | ControlSignalSnapshot::ProgramExpr { continuation, .. }
            | ControlSignalSnapshot::Block { continuation, .. }
            | ControlSignalSnapshot::Call { continuation, .. } => Some(continuation),
            _ => None,
        }
    }
}

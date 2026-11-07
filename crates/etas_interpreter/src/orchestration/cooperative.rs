use super::{
    CallSiteSnapshot, CallTargetSnapshot, ContinuationSnapshot, ExecutionProgramSnapshot,
    LocalsSnapshot, ValueSnapshot,
};
use etas_core::Span;
use etas_hir::{HirBlockId, HirExprId, HirItemId};

/// The saved input is separate from the stack: a ready task has an input while
/// an await/park waiter retains its continuation in the scheduler.
#[derive(Clone, Debug)]
pub(crate) enum MachineInputSnapshot {
    EnterProgram {
        owner: ExecutionProgramSnapshot,
        call: CallTargetSnapshot,
        args: Vec<ValueSnapshot>,
        span: Span,
    },
    Signal(ControlSignalSnapshot),
    ModelResult(Result<etas_host::ModelResponse, etas_host::HostError>),
    ToolResult(Result<etas_host::ToolResponse, etas_host::HostError>),
    SourceToolApproved,
}

#[derive(Clone, Debug)]
pub(crate) enum ControlSignalSnapshot {
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
    Block {
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
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidualActivationSnapshot {
    pub item: HirItemId,
    pub application: usize,
}

#[derive(Clone, Debug)]
pub(crate) enum TaskControlSnapshot {
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
    pub(crate) fn continuation_root(&self) -> Option<&ContinuationSnapshot> {
        match self {
            Self::Signal(
                ControlSignalSnapshot::Apply { continuation, .. }
                | ControlSignalSnapshot::Checkpoint { continuation, .. }
                | ControlSignalSnapshot::Expr { continuation, .. }
                | ControlSignalSnapshot::Block { continuation, .. }
                | ControlSignalSnapshot::Call { continuation, .. },
            ) => Some(continuation),
            _ => None,
        }
    }
}

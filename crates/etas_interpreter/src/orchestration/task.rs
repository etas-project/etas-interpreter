use super::{
    ContinuationSnapshot, ExecutionProgramSnapshot, HandlerSnapshot, MachineSnapshot,
    ModelExecutionPolicySnapshot, RetrySnapshot, ValueSnapshot,
};
use crate::driver::scheduler::{
    RuntimeResourceKey, TaskId, TaskIdentity, TaskSchedulerMetadataSnapshot, TaskScopeId,
};
use etas_core::Span;
use etas_types::TypeId;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(crate) struct TaskSchedulerSnapshot {
    pub metadata: TaskSchedulerMetadataSnapshot,
    pub tasks: BTreeMap<TaskId, ScheduledTaskSnapshot>,
    pub counters: TaskRunCountersSnapshot,
    pub diagnostics: Vec<etas_core::Diagnostic>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct TaskRunCountersSnapshot {
    pub step: u32,
    pub retry: u32,
    pub handler_scope: u32,
    pub host_request: u32,
    pub message: u32,
    pub cpu_dispatch: u64,
    pub cpu_completion: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct ScheduledTaskSnapshot {
    pub scope: TaskScopeId,
    pub program: ExecutionProgramSnapshot,
    pub local: TaskLocalSnapshot,
    pub machine: Option<MachineSnapshot>,
    pub handler_floor: usize,
    pub state: ScheduledTaskStateSnapshot,
    pub outcome: Option<TaskOutcomeSnapshot>,
    pub waiters: Vec<TaskId>,
    pub waiting_continuation: Option<TaskResumeSnapshot>,
    pub source_index: usize,
    pub active_slot: bool,
    pub cpu_pending: bool,
    pub resource_keys: Vec<RuntimeResourceKey>,
}

#[derive(Clone, Debug)]
pub(crate) struct TaskLocalSnapshot {
    pub parent: TaskIdentity,
    pub model_policy: ModelExecutionPolicySnapshot,
    pub handlers: HandlerSnapshot,
    pub retries: RetrySnapshot,
    pub cancellation: Option<TaskCancellationSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScheduledTaskStateSnapshot {
    Ready,
    WaitingTask(TaskId),
    WaitingScope(TaskScopeId),
    WaitingResource(u64),
    Completed,
    Raised,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub(crate) enum TaskOutcomeSnapshot {
    Completed(ValueSnapshot),
    // Terminal task errors have already crossed the child execution boundary.
    // Await supplies its own continuation when re-raising this typed payload.
    Raised {
        error_type: TypeId,
        payload: ValueSnapshot,
        span: Span,
    },
    Failed(crate::api::RunFailure),
    Cancelled(TaskCancellationSnapshot),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum TaskResumeSnapshot {
    Await(u32),
    Run(u32),
    Transfer {
        token: crate::value::ContinuationTokenValue,
        continuation: u32,
        output: TypeId,
        span: Span,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct AuxiliaryContinuationSnapshot {
    pub program: ExecutionProgramSnapshot,
    pub continuation: ContinuationSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TaskCancellationSnapshot {
    pub source: TaskIdentity,
    pub reason: TaskCancellationReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskCancellationReason {
    Requested,
    Interrupt,
    Terminate,
    OwnerDropped,
    ChildFailed,
    Deadline,
}

impl From<&etas_host::execution::CancellationReason> for TaskCancellationReason {
    fn from(reason: &etas_host::execution::CancellationReason) -> Self {
        use etas_host::execution::CancellationReason as R;
        match reason {
            R::Requested => Self::Requested,
            R::Interrupt => Self::Interrupt,
            R::Terminate => Self::Terminate,
            R::OwnerDropped => Self::OwnerDropped,
            R::ChildFailed => Self::ChildFailed,
            R::Deadline => Self::Deadline,
        }
    }
}
impl From<TaskCancellationReason> for etas_host::execution::CancellationReason {
    fn from(reason: TaskCancellationReason) -> Self {
        match reason {
            TaskCancellationReason::Requested => Self::Requested,
            TaskCancellationReason::Interrupt => Self::Interrupt,
            TaskCancellationReason::Terminate => Self::Terminate,
            TaskCancellationReason::OwnerDropped => Self::OwnerDropped,
            TaskCancellationReason::ChildFailed => Self::ChildFailed,
            TaskCancellationReason::Deadline => Self::Deadline,
        }
    }
}

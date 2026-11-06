use crate::{
    api::RunFailure,
    control::{Continuation, PendingPerform},
    eval::{TaskLocalState, machine::EvalMachine},
    value::{CheckedExecutionProgram, InterpValue},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TaskScopeId(pub(crate) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TaskId(pub(crate) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ContinuationId(pub(crate) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskIdentity {
    Root,
    Scheduled(TaskId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TaskHandle {
    pub(crate) owner: TaskScopeId,
    pub(crate) task: TaskId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskWait {
    Task(TaskId),
    Scope(TaskScopeId),
    Host(etas_host::execution::OperationId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskState {
    Ready,
    Running,
    Waiting(TaskWait),
    Completed,
    Raised,
    Failed,
    Cancelled,
}

impl TaskState {
    pub(crate) fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Raised | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) enum TaskOutcome {
    Completed(InterpValue),
    Raised(Box<PendingPerform>),
    Failed(RunFailure),
    Cancelled(etas_host::execution::CancellationCause),
}

impl TaskOutcome {
    pub(crate) fn state(&self) -> TaskState {
        match self {
            Self::Completed(_) => TaskState::Completed,
            Self::Raised(_) => TaskState::Raised,
            Self::Failed(_) => TaskState::Failed,
            Self::Cancelled(_) => TaskState::Cancelled,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskWaiter {
    Task(TaskId),
}

pub(crate) enum TaskResume {
    Await(Continuation),
    Run(Continuation),
    Transfer {
        token: crate::value::ContinuationTokenValue,
        continuation: Continuation,
        output: etas_types::TypeId,
        span: etas_core::Span,
    },
}

pub(crate) struct ScheduledTask {
    pub(crate) scope: TaskScopeId,
    pub(crate) machine: EvalMachine,
    pub(crate) local: TaskLocalState,
    pub(crate) program: Option<CheckedExecutionProgram>,
    pub(crate) handler_floor: usize,
    pub(crate) state: TaskState,
    pub(crate) outcome: Option<TaskOutcome>,
    pub(crate) waiters: Vec<TaskWaiter>,
    pub(crate) waiting_continuation: Option<TaskResume>,
    pub(crate) source_index: usize,
}

impl ScheduledTask {
    pub(crate) fn new(
        scope: TaskScopeId,
        machine: EvalMachine,
        local: TaskLocalState,
        program: Option<CheckedExecutionProgram>,
        handler_floor: usize,
        source_index: usize,
    ) -> Self {
        Self {
            scope,
            machine,
            local,
            program,
            handler_floor,
            state: TaskState::Ready,
            outcome: None,
            waiters: Vec::new(),
            waiting_continuation: None,
            source_index,
        }
    }
}

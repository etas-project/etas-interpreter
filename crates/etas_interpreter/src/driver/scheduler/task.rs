use crate::{
    api::RunFailure,
    control::{Continuation, PendingPerform},
    eval::{TaskLocalState, machine::EvalMachine},
    value::{CheckedExecutionProgram, InterpValue},
};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub(crate) struct TaskScopeId(pub(crate) u64);

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub(crate) struct TaskId(pub(crate) u64);

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub(crate) struct ContinuationId(pub(crate) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum TaskIdentity {
    Root,
    Scheduled(TaskId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TaskHandle {
    pub(crate) owner: TaskScopeId,
    pub(crate) task: TaskId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskWait {
    Task(TaskId),
    Scope(TaskScopeId),
    Completion(TaskScopeId),
    Host(etas_host::execution::OperationId),
    HostDrain,
    Cpu(crate::driver::cpu::CpuJobId),
    Resource(super::resource::ResourceGuardId),
    Handler(crate::orchestration::HandlerScopeId),
    HandlerOwner(TaskId),
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

#[derive(Clone, Debug)]
pub(crate) struct CheckedTaskStart {
    pub(crate) target: crate::control::CallTarget,
    pub(crate) result_type: etas_types::TypeId,
}

pub(crate) enum TaskResume {
    DeferredHandler {
        handler: crate::orchestration::HandlerScopeId,
    },
    Handler {
        continuation: Continuation,
        handler: crate::orchestration::HandlerScopeId,
    },
    Await(Continuation),
    Run(Continuation),
    Completion {
        continuation: Continuation,
        result_type: etas_types::TypeId,
        span: etas_core::Span,
    },
    Transfer {
        token: crate::value::ContinuationTokenValue,
        continuation: crate::eval::machine::ParkedContinuation,
        output: etas_types::TypeId,
        span: etas_core::Span,
    },
}

#[derive(Debug)]
pub(crate) enum RootTaskWait {
    Completion {
        scope: TaskScopeId,
        continuation: Continuation,
        result_type: etas_types::TypeId,
        span: etas_core::Span,
    },
    Ready(Box<crate::eval::machine::MachineInput>),
    Await {
        target: TaskHandle,
        continuation: Continuation,
    },
    Run {
        scope: TaskScopeId,
    },
    Transfer {
        target: TaskHandle,
        token: crate::value::ContinuationTokenValue,
        continuation: crate::eval::machine::ParkedContinuation,
        output: etas_types::TypeId,
        span: etas_core::Span,
    },
    Host(super::HostWait),
    Settling(TaskOutcome),
}

pub(crate) struct ScheduledTask {
    pub(crate) inherited_dynamic: (usize, usize),
    pub(crate) start: Option<std::sync::Arc<CheckedTaskStart>>,
    pub(crate) scope: TaskScopeId,
    pub(crate) machine: EvalMachine,
    pub(crate) local: TaskLocalState,
    pub(crate) program: Option<CheckedExecutionProgram>,
    pub(crate) handler_floor: usize,
    pub(crate) state: TaskState,
    pub(crate) outcome: Option<TaskOutcome>,
    pub(crate) waiters: Vec<TaskWaiter>,
    pub(crate) waiting_continuation: Option<TaskResume>,
    pub(crate) host_wait: Option<super::HostWait>,
    pub(crate) pending_outcome: Option<TaskOutcome>,
    pub(crate) source_index: usize,
    pub(crate) cpu_capsule: Option<crate::driver::cpu::CpuTaskCapsule>,
    pub(crate) active_slot: bool,
    pub(crate) resource_keys: Vec<super::resource::RuntimeResourceKey>,
}

impl ScheduledTask {
    pub(super) fn can_relay_handler(&self) -> bool {
        matches!(
            self.state,
            TaskState::Waiting(
                TaskWait::Task(_)
                    | TaskWait::Scope(_)
                    | TaskWait::Completion(_)
                    | TaskWait::Host(_)
            )
        ) || self.state == TaskState::Ready
            && self.machine.has_input()
            && !matches!(
                self.waiting_continuation,
                Some(TaskResume::DeferredHandler { .. })
            )
    }

    pub(crate) fn new(
        scope: TaskScopeId,
        machine: EvalMachine,
        local: TaskLocalState,
        program: Option<CheckedExecutionProgram>,
        handler_floor: usize,
        source_index: usize,
    ) -> Self {
        let inherited_dynamic = local.dynamic_depths();
        Self {
            inherited_dynamic,
            start: None,
            scope,
            machine,
            local,
            program,
            handler_floor,
            state: TaskState::Ready,
            outcome: None,
            waiters: Vec::new(),
            waiting_continuation: None,
            host_wait: None,
            pending_outcome: None,
            source_index,
            cpu_capsule: None,
            active_slot: false,
            resource_keys: Vec::new(),
        }
    }
}

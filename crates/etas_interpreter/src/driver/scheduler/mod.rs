mod completion;
mod continuation;
mod group;
mod host;
mod host_wait;
mod policy;
mod resource;
mod snapshot;
mod task;

pub(crate) use resource::{
    EffectIdentity, FrozenSelector, ResourceEvent, ResourceGuardId, RuntimeResourceKey,
};

use std::{
    collections::{BTreeMap, VecDeque},
    num::NonZeroUsize,
};

pub(crate) use completion::{CompletionDecision, TaskCompletion};
pub(crate) use continuation::{ConsumedContinuation, ContinuationTransfer};
pub(crate) use continuation::{
    ContinuationArenaSnapshot, ContinuationEntrySnapshot, ContinuationState,
};
pub(crate) use group::{TaskGroupPolicy, TaskScopeSnapshot};
pub(in crate::driver) use host::{CompletedTaskHost, HostOwner, ReadyTaskHost, TaskHostReactor};
pub(crate) use host_wait::HostWait;
pub(in crate::driver) use host_wait::{HostOperation, HostOperationScope};
pub(crate) use policy::{ReadyTaskSnapshot, SourcePolicySnapshot, SourceSchedulingPolicy};
pub(crate) use resource::ResourceGuardsSnapshot;
pub(crate) use snapshot::TaskSchedulerMetadataSnapshot;
pub(crate) use task::{
    CheckedTaskStart, ContinuationId, RootTaskWait, ScheduledTask, TaskHandle, TaskId,
    TaskIdentity, TaskOutcome, TaskResume, TaskScopeId, TaskState, TaskWait, TaskWaiter,
};

pub(crate) const MAX_CONCURRENCY: usize = etas_std::intrinsic::task::MAX_CONCURRENCY as usize;

use crate::{api::RunFailure, eval::machine::EvalMachine, value::InterpValue};

#[derive(Debug)]
pub(crate) enum SchedulerError {
    Host(etas_host::HostError),
    WrongOwner,
    UnknownTask,
    UnknownContinuation,
    ContinuationConsumed,
    WrongTask,
    InvalidTransition,
}

impl From<etas_host::HostError> for SchedulerError {
    fn from(value: etas_host::HostError) -> Self {
        Self::Host(value)
    }
}

impl std::fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host(error) => write!(f, "task execution scope: {error}"),
            Self::WrongOwner => f.write_str("task handle belongs to another scheduler scope"),
            Self::UnknownTask => f.write_str("task handle is stale"),
            Self::UnknownContinuation => f.write_str("continuation token is stale"),
            Self::ContinuationConsumed => f.write_str("continuation token was already consumed"),
            Self::WrongTask => f.write_str("continuation token belongs to another task"),
            Self::InvalidTransition => f.write_str("task state transition is invalid"),
        }
    }
}

impl std::error::Error for SchedulerError {}

#[derive(Clone, Debug)]
pub(crate) enum TaskWakeup {
    Completion {
        scope: TaskScopeId,
        event: TaskCompletion,
    },
    Task {
        completed: TaskHandle,
        outcome: TaskOutcome,
    },
    Scope {
        scope: TaskScopeId,
        outcome: TaskOutcome,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum AwaitDecision {
    Ready(TaskOutcome),
    Parked,
}

#[derive(Clone, Debug)]
pub(crate) enum ScopeDecision {
    Ready(TaskOutcome),
    Parked,
}

/// One structured task scope. Tasks remain owned here after their language
/// handles are dropped, and terminal outcomes remain cached for repeat awaits.
pub(crate) struct TaskScheduler {
    next_scope: u64,
    next_task: u64,
    groups: BTreeMap<TaskScopeId, group::TaskGroup>,
    tasks: BTreeMap<TaskId, ScheduledTask>,
    ready: VecDeque<TaskId>,
    deferred_handlers: BTreeMap<TaskId, TaskId>,
    enqueue_orders: BTreeMap<TaskId, u64>,
    next_enqueue_order: BTreeMap<TaskScopeId, u64>,
    policies: BTreeMap<TaskScopeId, SourceSchedulingPolicy>,
    wakeups: BTreeMap<TaskId, VecDeque<TaskWakeup>>,
    cancellation_events: Vec<(TaskHandle, etas_host::execution::CancellationReason)>,
    terminal_events: Vec<(TaskHandle, TaskState)>,
    continuations: continuation::ContinuationArena,
    resources: resource::ResourceGuards,
    checkpoint_barrier: bool,
}

impl TaskScheduler {
    pub(crate) fn new(owner: TaskScopeId, concurrency: NonZeroUsize) -> Self {
        let mut groups = BTreeMap::new();
        groups.insert(
            owner,
            group::TaskGroup::new(None, concurrency, TaskGroupPolicy::Run),
        );
        Self {
            next_scope: owner.0.checked_add(1).expect("task scope id exhausted"),
            next_task: 0,
            groups,
            tasks: BTreeMap::new(),
            ready: VecDeque::new(),
            deferred_handlers: BTreeMap::new(),
            enqueue_orders: BTreeMap::new(),
            next_enqueue_order: BTreeMap::new(),
            policies: BTreeMap::new(),
            wakeups: BTreeMap::new(),
            cancellation_events: Vec::new(),
            terminal_events: Vec::new(),
            continuations: continuation::ContinuationArena::new(),
            resources: Default::default(),
            checkpoint_barrier: false,
        }
    }

    pub(crate) fn create_scope(
        &mut self,
        parent: TaskScopeId,
        concurrency: NonZeroUsize,
    ) -> TaskScopeId {
        self.create_scope_with_policy(parent, concurrency, TaskGroupPolicy::Run)
    }

    pub(crate) fn create_scope_with_policy(
        &mut self,
        parent: TaskScopeId,
        concurrency: NonZeroUsize,
        policy: TaskGroupPolicy,
    ) -> TaskScopeId {
        assert!(
            self.groups.contains_key(&parent),
            "parent task scope must exist"
        );
        let id = TaskScopeId(self.next_scope);
        self.next_scope = self
            .next_scope
            .checked_add(1)
            .expect("task scope id exhausted");
        assert!(
            self.groups
                .insert(id, group::TaskGroup::new(Some(parent), concurrency, policy))
                .is_none(),
            "task scope id must be unique"
        );
        id
    }

    pub(crate) fn allocate_continuation(
        &mut self,
        owner: TaskScopeId,
        origin: TaskIdentity,
        continuation: crate::control::Continuation,
        output: etas_types::TypeId,
        program: Option<crate::value::CheckedExecutionProgram>,
        binder: Option<etas_hir::SymbolId>,
    ) -> crate::value::ContinuationTokenValue {
        assert!(
            self.groups.contains_key(&owner),
            "continuation scope must exist"
        );
        self.continuations
            .allocate(owner, origin, continuation, output, program, binder)
    }

    pub(crate) fn capture_continuations(
        &self,
        context: &mut crate::eval::machine::snapshot::CaptureContext,
        capture: impl FnMut(
            TaskIdentity,
            Option<&crate::value::CheckedExecutionProgram>,
            &crate::control::Continuation,
        ) -> Result<u32, String>,
    ) -> Result<ContinuationArenaSnapshot, String> {
        self.continuations.capture(context, capture)
    }

    pub(crate) fn consume_continuation(
        &mut self,
        token: crate::value::ContinuationTokenValue,
        origin: TaskIdentity,
        transfer: ContinuationTransfer,
    ) -> Result<ConsumedContinuation, SchedulerError> {
        self.continuations.consume(token, origin, transfer)
    }

    pub(crate) fn dispose_continuation(
        &mut self,
        token: crate::value::ContinuationTokenValue,
        origin: TaskIdentity,
    ) -> Result<(), SchedulerError> {
        self.continuations.dispose(token, origin)
    }

    pub(crate) fn continuation_program(
        &self,
        token: crate::value::ContinuationTokenValue,
    ) -> Result<Option<crate::value::CheckedExecutionProgram>, SchedulerError> {
        self.continuations.program(token)
    }

    pub(crate) fn spawn(
        &mut self,
        parent: &crate::eval::TaskLocalState,
        machine: EvalMachine,
        program: Option<crate::value::CheckedExecutionProgram>,
        handler_floor: usize,
        source_index: usize,
    ) -> Result<TaskHandle, SchedulerError> {
        self.spawn_in_scope(
            parent,
            parent.task_scope,
            machine,
            program,
            handler_floor,
            source_index,
        )
    }

    pub(crate) fn spawn_in_scope(
        &mut self,
        parent: &crate::eval::TaskLocalState,
        scope: TaskScopeId,
        machine: EvalMachine,
        program: Option<crate::value::CheckedExecutionProgram>,
        handler_floor: usize,
        source_index: usize,
    ) -> Result<TaskHandle, SchedulerError> {
        assert!(
            !self.checkpoint_barrier,
            "task admission is paused by checkpoint barrier"
        );
        if !self.groups.contains_key(&scope) {
            return Err(SchedulerError::WrongOwner);
        }
        let id = TaskId(self.next_task);
        let local = parent.child_in_scope(scope, id)?;
        self.next_task = self.next_task.checked_add(1).expect("task id exhausted");
        self.tasks.insert(
            id,
            ScheduledTask::new(scope, machine, local, program, handler_floor, source_index),
        );
        self.groups
            .get_mut(&scope)
            .expect("validated task scope")
            .tasks
            .insert(id);
        self.enqueue(id);
        Ok(TaskHandle {
            owner: scope,
            task: id,
        })
    }

    pub(crate) fn set_scope_owner(
        &mut self,
        scope: TaskScopeId,
        owner: TaskHandle,
        logical_identity: TaskIdentity,
    ) -> Result<(), SchedulerError> {
        self.resolve_id(owner)?;
        if owner.owner != scope {
            return Err(SchedulerError::WrongOwner);
        }
        let group = self
            .groups
            .get_mut(&scope)
            .ok_or(SchedulerError::WrongOwner)?;
        if group.owner.replace(owner.task).is_some() {
            return Err(SchedulerError::InvalidTransition);
        }
        self.tasks
            .get_mut(&owner.task)
            .expect("scope owner task")
            .local
            .logical_identity = logical_identity;
        Ok(())
    }

    pub(crate) fn attach_cpu_capsule(
        &mut self,
        handle: TaskHandle,
        mut capsule: crate::driver::cpu::CpuTaskCapsule,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let task = self.tasks.get_mut(&id).expect("resolved task");
        capsule.contract.execution = task.local.execution.clone();
        if task.state != TaskState::Ready || task.cpu_capsule.replace(capsule).is_some() {
            return Err(SchedulerError::InvalidTransition);
        }
        Ok(())
    }

    pub(crate) fn attach_start(
        &mut self,
        handle: TaskHandle,
        call: CheckedTaskStart,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let task = self.tasks.get_mut(&id).expect("resolved task");
        assert!(task.start.is_none(), "task start origin is assigned once");
        task.start = Some(std::sync::Arc::new(call));
        Ok(())
    }

    pub(crate) fn state(&self, handle: TaskHandle) -> Result<TaskState, SchedulerError> {
        Ok(self.resolve(handle)?.state)
    }

    pub(crate) fn attach_resources(
        &mut self,
        handle: TaskHandle,
        mut keys: Vec<RuntimeResourceKey>,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        keys.sort();
        keys.dedup();
        self.tasks
            .get_mut(&id)
            .expect("resolved task")
            .resource_keys = keys;
        Ok(())
    }

    pub(crate) fn guard_scope(&mut self, scope: TaskScopeId) {
        self.groups
            .get_mut(&scope)
            .expect("created scope")
            .guard_resources = true;
    }

    pub(crate) fn requires_resource_guards(&self, mut scope: TaskScopeId) -> bool {
        loop {
            let group = &self.groups[&scope];
            if group.guard_resources {
                return true;
            }
            let Some(parent) = group.parent else {
                return false;
            };
            scope = parent;
        }
    }

    /// Unknown selectors serialize the invocation, including already admitted
    /// Host waits. Parent scope waits still release their slot for nested tasks.
    pub(crate) fn serialize_invocation(&mut self) -> TaskScopeId {
        let root = *self
            .groups
            .iter()
            .find(|(_, group)| group.parent.is_none())
            .expect("root scope")
            .0;
        let group = self.groups.get_mut(&root).unwrap();
        group.concurrency = NonZeroUsize::MIN;
        group.serialize_host_actions = true;
        group.serial_task_admission = true;
        let retain = self
            .tasks
            .iter()
            .filter(|(id, task)| {
                self.resources.holders.contains_key(id)
                    && !task.active_slot
                    && matches!(
                        task.state,
                        TaskState::Ready | TaskState::Waiting(TaskWait::Host(_))
                    )
            })
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in retain {
            let task = self.tasks.get_mut(&id).unwrap();
            task.active_slot = true;
            let scope = task.scope;
            self.change_active(scope, true);
        }
        root
    }

    pub(crate) fn take_resource_events(&mut self) -> Vec<resource::ResourceEvent> {
        std::mem::take(&mut self.resources.events)
    }

    pub(crate) fn task_scope(&self, id: TaskId) -> TaskScopeId {
        self.tasks[&id].scope
    }

    pub(crate) fn outcome(
        &self,
        handle: TaskHandle,
    ) -> Result<Option<&TaskOutcome>, SchedulerError> {
        Ok(self.resolve(handle)?.outcome.as_ref())
    }

    pub(crate) fn scope_resolution(
        &self,
        scope: TaskScopeId,
    ) -> Result<(&'static str, Option<TaskId>), SchedulerError> {
        let group = self.groups.get(&scope).ok_or(SchedulerError::WrongOwner)?;
        if let Some(winner) = group.winner {
            return Ok(("first_success", Some(winner)));
        }
        if let Some(primary) = group.primary_failure {
            let decision = if group.policy == TaskGroupPolicy::TryJoin
                && matches!(self.tasks[&primary].outcome, Some(TaskOutcome::Raised(_)))
            {
                "captured_primary_error"
            } else {
                "primary_failure"
            };
            return Ok((decision, Some(primary)));
        }
        if let Some(owner) = group.owner {
            return Ok(("owner_completed", Some(owner)));
        }
        Ok(("all_completed", None))
    }

    pub(crate) fn take_cancellation_events(
        &mut self,
    ) -> Vec<(TaskHandle, etas_host::execution::CancellationReason)> {
        std::mem::take(&mut self.cancellation_events)
    }

    pub(crate) fn take_terminal_events(
        &mut self,
        mut ready: impl FnMut(TaskHandle) -> bool,
    ) -> Vec<(TaskHandle, TaskState)> {
        let mut events = Vec::new();
        self.terminal_events.retain(|(handle, state)| {
            if ready(*handle) {
                events.push((*handle, *state));
                false
            } else {
                true
            }
        });
        events
    }

    /// FIFO admission. Stale queue entries from cancellation are skipped.
    pub(crate) fn dispatch_ready(&mut self) -> Option<TaskHandle> {
        if self.checkpoint_barrier {
            return None;
        }
        let queued = self.ready.len();
        for _ in 0..queued {
            let id = self.ready.pop_front().expect("bounded ready queue scan");
            let task = self.tasks.get(&id).expect("ready task must exist");
            if task.state != TaskState::Ready {
                continue;
            }
            let scope = task.scope;
            let active_slot = task.active_slot;
            if !active_slot && !self.can_admit(scope) {
                self.ready.push_back(id);
                continue;
            }
            if !self.resources.acquire(id, &task.resource_keys) {
                let guard = resource::ResourceGuardId(id.0);
                self.tasks.get_mut(&id).unwrap().state =
                    TaskState::Waiting(TaskWait::Resource(guard));
                self.resources.waiting.push_back(id);
                self.resources
                    .events
                    .push(resource::ResourceEvent::Waiting(id, guard));
                continue;
            }
            let task = self.tasks.get_mut(&id).expect("ready task must exist");
            task.state = TaskState::Running;
            self.enqueue_orders.remove(&id);
            if !active_slot {
                task.active_slot = true;
                self.change_active(scope, true);
            }
            return Some(TaskHandle {
                owner: scope,
                task: id,
            });
        }
        None
    }

    pub(crate) fn begin_checkpoint_barrier(&mut self) {
        assert!(!self.checkpoint_barrier, "checkpoint barriers cannot nest");
        self.checkpoint_barrier = true;
    }

    pub(crate) fn end_checkpoint_barrier(&mut self) {
        assert!(self.checkpoint_barrier, "checkpoint barrier must be active");
        self.checkpoint_barrier = false;
    }

    pub(crate) fn take_checkpoint_task(&mut self, handle: TaskHandle) -> ScheduledTask {
        assert!(
            self.checkpoint_barrier,
            "checkpoint task access requires a barrier"
        );
        self.take_ready_task(handle)
    }

    pub(crate) fn restore_checkpoint_task(&mut self, handle: TaskHandle, task: ScheduledTask) {
        assert!(
            self.checkpoint_barrier,
            "checkpoint task access requires a barrier"
        );
        self.restore_ready_task(handle, task);
    }

    /// Commit a completed input without running source or changing admission,
    /// FIFO order, active slots, resource permits, or waiter ownership.
    /// Host settlement can address a suspended wait while its owner is in a
    /// Handler arm or already terminal; it never executes the owner's source.
    pub(crate) fn take_host_owner(&mut self, handle: TaskHandle) -> ScheduledTask {
        let task = self.tasks.remove(&handle.task).expect("Host owner exists");
        assert_eq!(task.scope, handle.owner);
        task
    }
    pub(crate) fn restore_host_owner(&mut self, handle: TaskHandle, task: ScheduledTask) {
        assert_eq!(task.scope, handle.owner);
        assert!(
            self.tasks.insert(handle.task, task).is_none(),
            "Host owner restored once"
        );
    }

    pub(crate) fn take_ready_task(&mut self, handle: TaskHandle) -> ScheduledTask {
        let task = self.tasks.remove(&handle.task).expect("ready task exists");
        assert_eq!(task.scope, handle.owner);
        assert_eq!(task.state, TaskState::Ready);
        task
    }

    pub(crate) fn restore_ready_task(&mut self, handle: TaskHandle, task: ScheduledTask) {
        assert_eq!(task.scope, handle.owner);
        assert_eq!(task.state, TaskState::Ready);
        assert!(
            self.tasks.insert(handle.task, task).is_none(),
            "ready task restored once"
        );
    }

    pub(crate) fn take_running(
        &mut self,
        handle: TaskHandle,
    ) -> Result<ScheduledTask, SchedulerError> {
        let id = self.resolve_id(handle)?;
        if self.tasks[&id].state != TaskState::Running {
            return Err(SchedulerError::InvalidTransition);
        }
        self.tasks.remove(&id).ok_or(SchedulerError::UnknownTask)
    }

    pub(crate) fn restore_running(
        &mut self,
        handle: TaskHandle,
        task: ScheduledTask,
    ) -> Result<(), SchedulerError> {
        if handle.owner != task.scope || task.state != TaskState::Running {
            return Err(SchedulerError::InvalidTransition);
        }
        if self.tasks.insert(handle.task, task).is_some() {
            return Err(SchedulerError::InvalidTransition);
        }
        Ok(())
    }

    pub(crate) fn requeue(&mut self, handle: TaskHandle) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let retain_slot = self.serial_task_admission(self.tasks[&id].scope);
        let task = self.tasks.get_mut(&id).expect("resolved task");
        if task.state != TaskState::Running {
            return Err(SchedulerError::InvalidTransition);
        }
        task.state = TaskState::Ready;
        let scope = task.scope;
        if !retain_slot && task.active_slot {
            task.active_slot = false;
            self.change_active(scope, false);
        }
        self.enqueue(id);
        Ok(())
    }

    /// Atomically observes a target and either returns its cached outcome or
    /// installs the waiter before the target can be completed by this arena.
    pub(crate) fn await_task(
        &mut self,
        waiter: TaskHandle,
        target: TaskHandle,
    ) -> Result<AwaitDecision, SchedulerError> {
        let waiter_id = self.resolve_id(waiter)?;
        let target_id = self.resolve_id(target)?;
        if self.tasks[&waiter_id].state != TaskState::Running {
            return Err(SchedulerError::InvalidTransition);
        }
        if let Some(outcome) = self.tasks[&target_id].outcome.clone() {
            return Ok(AwaitDecision::Ready(outcome));
        }
        self.tasks
            .get_mut(&target_id)
            .expect("resolved target")
            .waiters
            .push(TaskWaiter::Task(waiter_id));
        self.tasks
            .get_mut(&waiter_id)
            .expect("resolved waiter")
            .state = TaskState::Waiting(TaskWait::Task(target_id));
        let scope = self.tasks[&waiter_id].scope;
        if self.tasks[&waiter_id].active_slot {
            self.tasks
                .get_mut(&waiter_id)
                .expect("resolved waiter")
                .active_slot = false;
            self.change_active(scope, false);
        }
        Ok(AwaitDecision::Parked)
    }

    pub(crate) fn await_scope(
        &mut self,
        waiter: TaskHandle,
        scope: TaskScopeId,
    ) -> Result<ScopeDecision, SchedulerError> {
        let waiter_id = self.resolve_id(waiter)?;
        if self.tasks[&waiter_id].state != TaskState::Running {
            return Err(SchedulerError::InvalidTransition);
        }
        if let Some(outcome) = self.scope_outcome(scope)? {
            return Ok(ScopeDecision::Ready(outcome));
        }
        self.groups
            .get_mut(&scope)
            .ok_or(SchedulerError::WrongOwner)?
            .waiters
            .push(waiter_id);
        self.tasks
            .get_mut(&waiter_id)
            .expect("resolved waiter")
            .state = TaskState::Waiting(TaskWait::Scope(scope));
        let waiter_scope = self.tasks[&waiter_id].scope;
        if self.tasks[&waiter_id].active_slot {
            self.tasks
                .get_mut(&waiter_id)
                .expect("resolved waiter")
                .active_slot = false;
            self.change_active(waiter_scope, false);
        }
        Ok(ScopeDecision::Parked)
    }

    pub(crate) fn scope_outcome(
        &self,
        scope: TaskScopeId,
    ) -> Result<Option<TaskOutcome>, SchedulerError> {
        let group = self.groups.get(&scope).ok_or(SchedulerError::WrongOwner)?;
        // A running machine is leased out of the task table. It still owns
        // its admission slot, so the scope cannot settle while that lease lives.
        if group.active != 0 {
            return Ok(None);
        }
        if group
            .tasks
            .iter()
            .any(|task| !self.tasks[task].state.is_terminal())
        {
            return Ok(None);
        }
        let ordered = self.ordered_tasks(group);
        let first_cancelled = || {
            ordered
                .iter()
                .find_map(|task| match &self.tasks[task].outcome {
                    Some(TaskOutcome::Cancelled(cause)) => {
                        Some(TaskOutcome::Cancelled(cause.clone()))
                    }
                    _ => None,
                })
        };
        let output = match group.policy {
            TaskGroupPolicy::Run => {
                if let Some(primary) = group.primary_failure {
                    self.tasks[&primary].outcome.clone()
                } else {
                    let owner = group.owner.ok_or(SchedulerError::InvalidTransition)?;
                    self.tasks[&owner].outcome.clone()
                }
            }
            TaskGroupPolicy::JoinTuple | TaskGroupPolicy::JoinArray | TaskGroupPolicy::Map => {
                if let Some(primary) = group.primary_failure {
                    self.tasks[&primary].outcome.clone()
                } else if let Some(cancelled) = first_cancelled() {
                    Some(cancelled)
                } else {
                    let values = self.completed_values(&ordered)?;
                    let value = if group.policy == TaskGroupPolicy::JoinTuple {
                        InterpValue::Tuple(crate::value::SharedFields::new(values))
                    } else {
                        InterpValue::Array(crate::value::ArrayValue::new(values))
                    };
                    Some(TaskOutcome::Completed(value))
                }
            }
            TaskGroupPolicy::TryJoin => {
                if let Some(primary) = group.primary_failure {
                    match self.tasks[&primary].outcome.as_ref() {
                        Some(TaskOutcome::Raised(perform)) => Some(TaskOutcome::Completed(
                            result_value("Err", error_payload(perform)),
                        )),
                        outcome => outcome.cloned(),
                    }
                } else if let Some(cancelled) = first_cancelled() {
                    Some(cancelled)
                } else {
                    Some(TaskOutcome::Completed(result_value(
                        "Ok",
                        InterpValue::Array(crate::value::ArrayValue::new(
                            self.completed_values(&ordered)?,
                        )),
                    )))
                }
            }
            TaskGroupPolicy::Collect | TaskGroupPolicy::MapCollect => {
                if let Some(failure) = ordered.iter().find_map(|task| {
                    matches!(self.tasks[task].outcome, Some(TaskOutcome::Failed(_)))
                        .then_some(*task)
                }) {
                    self.tasks[&failure].outcome.clone()
                } else if ordered.iter().all(|task| {
                    matches!(
                        self.tasks[task].outcome,
                        Some(TaskOutcome::Completed(_) | TaskOutcome::Raised(_))
                    )
                }) {
                    let values = ordered
                        .iter()
                        .map(|task| {
                            match self.tasks[task].outcome.as_ref().expect("settled task") {
                                TaskOutcome::Completed(value) => result_value("Ok", value.clone()),
                                TaskOutcome::Raised(perform) => {
                                    result_value("Err", error_payload(perform))
                                }
                                _ => unreachable!(
                                    "collect accepts completed values and typed errors"
                                ),
                            }
                        })
                        .collect();
                    Some(TaskOutcome::Completed(InterpValue::Array(
                        crate::value::ArrayValue::new(values),
                    )))
                } else {
                    first_cancelled()
                }
            }
            TaskGroupPolicy::Race => {
                if let Some(winner) = group.winner {
                    self.tasks[&winner].outcome.clone()
                } else if let Some(primary) = group.primary_failure {
                    self.tasks[&primary].outcome.clone()
                } else {
                    first_cancelled()
                }
            }
        };
        output.ok_or(SchedulerError::InvalidTransition).map(Some)
    }

    pub(crate) fn task_logical_identity(
        &self,
        identity: TaskIdentity,
    ) -> Result<TaskIdentity, SchedulerError> {
        match identity {
            TaskIdentity::Root => Ok(TaskIdentity::Root),
            TaskIdentity::Scheduled(id) => self
                .tasks
                .get(&id)
                .map(|task| task.local.logical_identity)
                .ok_or(SchedulerError::UnknownTask),
        }
    }

    pub(crate) fn task_parent(&self, id: TaskId) -> Option<TaskIdentity> {
        self.tasks.get(&id)?.local.parent
    }

    pub(crate) fn arm_owned_continuations(
        &self,
        task: TaskId,
    ) -> Vec<(
        crate::value::CheckedExecutionProgram,
        crate::control::Continuation,
    )> {
        self.continuations
            .arm_owned_for(TaskIdentity::Scheduled(task))
            .map(|(program, continuation)| (program.clone(), continuation.clone()))
            .collect()
    }

    pub(crate) fn wait_for_handler_owner(
        &mut self,
        handle: TaskHandle,
        owner: TaskId,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let task = self.tasks.get_mut(&id).expect("resolved task");
        if task.state != TaskState::Running {
            return Err(SchedulerError::InvalidTransition);
        }
        task.state = TaskState::Waiting(TaskWait::HandlerOwner(owner));
        let scope = task.scope;
        if task.active_slot {
            task.active_slot = false;
            self.change_active(scope, false);
        }
        self.deferred_handlers.insert(id, owner);
        Ok(())
    }

    pub(crate) fn wake_exposed_handlers(&mut self) -> Result<(), SchedulerError> {
        let mut ready = Vec::new();
        let mut cancelled = Vec::new();
        self.deferred_handlers.retain(|id, owner| {
            let task = &self.tasks[id];
            if task.state != TaskState::Waiting(TaskWait::HandlerOwner(*owner)) {
                return false;
            }
            let Some(TaskResume::DeferredHandler { handler }) = task.waiting_continuation else {
                unreachable!("deferred task owns its selected Handler")
            };
            let owner = &self.tasks[owner];
            if owner.state.is_terminal() {
                cancelled.push(TaskHandle {
                    owner: task.scope,
                    task: *id,
                });
                false
            } else if owner.can_relay_handler() && owner.local.has_owned_handler(handler) {
                ready.push(*id);
                false
            } else {
                true
            }
        });
        for id in ready {
            self.tasks.get_mut(&id).expect("waiting child").state = TaskState::Ready;
            self.enqueue(id);
        }
        for handle in cancelled {
            self.cancel(
                handle,
                etas_host::execution::CancellationReason::OwnerDropped,
            )?;
        }
        Ok(())
    }

    pub(crate) fn take_handler_caller(
        &mut self,
        id: TaskId,
        inherited: &[crate::orchestration::ActiveHandlerRecord],
    ) -> Option<(TaskHandle, ScheduledTask)> {
        let task = self.tasks.get(&id)?;
        if !task.local.owns_inherited_handler(inherited)
            && !(task.state == TaskState::Ready
                && self
                    .continuations
                    .arm_owned_for(TaskIdentity::Scheduled(id))
                    .next()
                    .is_some())
        {
            return None;
        }
        if !task.can_relay_handler() {
            return None;
        }
        let handle = TaskHandle {
            owner: task.scope,
            task: id,
        };
        Some((handle, self.tasks.remove(&id).expect("handler caller")))
    }

    pub(crate) fn restore_handler_caller(
        &mut self,
        handle: TaskHandle,
        mut task: ScheduledTask,
        dispatch: bool,
        active: TaskId,
    ) {
        let previous = task.state;
        if dispatch {
            match previous {
                TaskState::Waiting(TaskWait::Completion(scope)) => {
                    self.groups
                        .get_mut(&scope)
                        .expect("completion scope")
                        .completion_waiter = None;
                }
                TaskState::Waiting(TaskWait::Task(target)) if target != active => self
                    .tasks
                    .get_mut(&target)
                    .expect("awaited task")
                    .waiters
                    .retain(|waiter| !matches!(waiter, TaskWaiter::Task(id) if *id == handle.task)),
                TaskState::Waiting(TaskWait::Scope(scope)) => self
                    .groups
                    .get_mut(&scope)
                    .expect("awaited scope")
                    .waiters
                    .retain(|id| *id != handle.task),
                _ => (),
            }
            task.state = TaskState::Ready;
        }
        assert!(
            self.tasks.insert(handle.task, task).is_none(),
            "handler caller remains unique"
        );
        if dispatch && previous != TaskState::Ready {
            self.enqueue(handle.task);
        }
    }

    pub(crate) fn wake_task_wait(
        &mut self,
        handle: TaskHandle,
        wakeup: TaskWakeup,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        self.wakeups.entry(id).or_default().push_back(wakeup);
        self.requeue(handle)
    }

    pub(crate) fn wait_for_handler(
        &mut self,
        handle: TaskHandle,
        handler: crate::orchestration::HandlerScopeId,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let task = self.tasks.get_mut(&id).expect("resolved task");
        if task.state != TaskState::Running {
            return Err(SchedulerError::InvalidTransition);
        }
        task.state = TaskState::Waiting(TaskWait::Handler(handler));
        let scope = task.scope;
        if task.active_slot {
            task.active_slot = false;
            self.change_active(scope, false);
        }
        Ok(())
    }

    pub(crate) fn resume_handled_action(
        &mut self,
        handle: TaskHandle,
        expected_handler: crate::orchestration::HandlerScopeId,
        value: InterpValue,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let task = self.tasks.get_mut(&id).expect("resolved task");
        let TaskState::Waiting(TaskWait::Handler(handler)) = task.state else {
            return Err(SchedulerError::InvalidTransition);
        };
        if handler != expected_handler {
            return Err(SchedulerError::InvalidTransition);
        }
        let Some(TaskResume::Handler {
            continuation,
            handler: expected,
        }) = task.waiting_continuation.take()
        else {
            return Err(SchedulerError::InvalidTransition);
        };
        if expected != handler {
            return Err(SchedulerError::InvalidTransition);
        }
        task.machine
            .resume(crate::control::ControlSignal::pending_continuation(
                crate::control::PendingContinuation {
                    continuation,
                    input: crate::control::ContinuationInput::Value(value),
                },
            ));
        task.state = TaskState::Ready;
        self.enqueue(id);
        Ok(())
    }

    pub(crate) fn close_handler(
        &mut self,
        handler: crate::orchestration::HandlerScopeId,
    ) -> Result<(), SchedulerError> {
        if !self
            .tasks
            .values()
            .any(|task| task.state == TaskState::Waiting(TaskWait::Handler(handler)) ||
                matches!(task.waiting_continuation, Some(TaskResume::DeferredHandler { handler: reserved }) if reserved == handler))
        {
            return Ok(());
        }
        // `finish` disposes requests delegated to this lexical boundary. Only
        // descendants that inherited it are affected; the task running the arm
        // can continue after the handled expression.
        let tasks = self
            .tasks
            .iter()
            .filter(|(_, task)| {
                task.local
                    .inherited_handler_records()
                    .iter()
                    .any(|scope| scope.id == handler)
            })
            .map(|(id, task)| TaskHandle {
                owner: task.scope,
                task: *id,
            })
            .collect::<Vec<_>>();
        for task in tasks {
            self.cancel(task, etas_host::execution::CancellationReason::OwnerDropped)?;
        }
        Ok(())
    }

    pub(crate) fn wait_for_host(
        &mut self,
        handle: TaskHandle,
        operation: etas_host::execution::OperationId,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let retain_slot = self.scope_serialized(self.tasks[&id].scope);
        let task = self.tasks.get_mut(&id).expect("resolved task");
        if task.state != TaskState::Running {
            return Err(SchedulerError::InvalidTransition);
        }
        task.state = TaskState::Waiting(TaskWait::Host(operation));
        let scope = task.scope;
        if !retain_slot && task.active_slot {
            task.active_slot = false;
            self.change_active(scope, false);
        }
        Ok(())
    }

    pub(crate) fn wait_for_cpu(
        &mut self,
        handle: TaskHandle,
        job: crate::driver::cpu::CpuJobId,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let task = self.tasks.get_mut(&id).expect("resolved task");
        if task.state != TaskState::Running {
            return Err(SchedulerError::InvalidTransition);
        }
        task.state = TaskState::Waiting(TaskWait::Cpu(job));
        Ok(())
    }

    pub(crate) fn take_cpu_completion(
        &mut self,
        handle: TaskHandle,
        job: crate::driver::cpu::CpuJobId,
    ) -> Result<ScheduledTask, SchedulerError> {
        let id = self.resolve_id(handle)?;
        let task = self.tasks.get_mut(&id).expect("resolved task");
        if task.state != TaskState::Waiting(TaskWait::Cpu(job)) {
            return Err(SchedulerError::InvalidTransition);
        }
        task.state = TaskState::Running;
        self.tasks.remove(&id).ok_or(SchedulerError::UnknownTask)
    }

    pub(crate) fn wake_from_host(
        &mut self,
        handle: TaskHandle,
        operation: etas_host::execution::OperationId,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let task = self.tasks.get_mut(&id).expect("resolved task");
        if task.state != TaskState::Waiting(TaskWait::Host(operation)) {
            return Err(SchedulerError::InvalidTransition);
        }
        task.state = TaskState::Ready;
        self.enqueue(id);
        Ok(())
    }

    pub(crate) fn complete(
        &mut self,
        handle: TaskHandle,
        value: InterpValue,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let scope = self.tasks[&id].scope;
        let race_winner = self.groups[&scope].policy == TaskGroupPolicy::Race
            && self.groups[&scope].winner.is_none();
        if race_winner {
            self.groups.get_mut(&scope).expect("task scope").winner = Some(id);
        }
        self.terminal(handle, TaskOutcome::Completed(value))
    }

    pub(crate) fn raise(
        &mut self,
        handle: TaskHandle,
        perform: crate::control::PendingPerform,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let scope = self.tasks[&id].scope;
        self.record_primary_failure(scope, id);
        self.terminal(handle, TaskOutcome::Raised(Box::new(perform)))
    }

    pub(crate) fn fail(
        &mut self,
        handle: TaskHandle,
        failure: RunFailure,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let scope = self.tasks[&id].scope;
        self.record_primary_failure(scope, id);
        self.terminal(handle, TaskOutcome::Failed(failure))
    }

    pub(crate) fn apply_group_policy(&mut self, handle: TaskHandle) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let scope = self.tasks[&id].scope;
        let outcome = self.tasks[&id]
            .outcome
            .as_ref()
            .or(self.tasks[&id].pending_outcome.as_ref())
            .ok_or(SchedulerError::InvalidTransition)?;
        let reason = match outcome {
            TaskOutcome::Completed(_)
                if self.groups[&scope].policy == TaskGroupPolicy::Race
                    && self.groups[&scope].winner == Some(id) =>
            {
                Some(etas_host::execution::CancellationReason::Requested)
            }
            TaskOutcome::Raised(_)
                if matches!(
                    self.groups[&scope].policy,
                    TaskGroupPolicy::Run
                        | TaskGroupPolicy::JoinTuple
                        | TaskGroupPolicy::JoinArray
                        | TaskGroupPolicy::TryJoin
                        | TaskGroupPolicy::Map
                ) =>
            {
                Some(etas_host::execution::CancellationReason::ChildFailed)
            }
            TaskOutcome::Failed(_) => Some(etas_host::execution::CancellationReason::ChildFailed),
            _ => None,
        };
        if let Some(reason) = reason {
            self.cancel_scope_except(scope, reason, id)?;
        }
        Ok(())
    }

    pub(crate) fn cancel(
        &mut self,
        handle: TaskHandle,
        reason: etas_host::execution::CancellationReason,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let task = self.tasks.get_mut(&id).expect("resolved task");
        if task.state.is_terminal() {
            return Ok(());
        }
        let already_cancelled = task.local.execution.signal()?.cause()?.is_some();
        task.local.execution.cancel_source().stop(reason.clone())?;
        if !already_cancelled {
            self.cancellation_events.push((handle, reason));
        }
        if matches!(
            task.state,
            TaskState::Waiting(TaskWait::Host(_) | TaskWait::HostDrain)
        ) {
            return Ok(());
        }
        let cause = task
            .local
            .execution
            .signal()?
            .cause()?
            .expect("cancelled task must publish its cause");
        self.terminal(handle, TaskOutcome::Cancelled(cause))
    }

    pub(crate) fn cancelled(
        &mut self,
        handle: TaskHandle,
        cause: etas_host::execution::CancellationCause,
    ) -> Result<(), SchedulerError> {
        self.terminal(handle, TaskOutcome::Cancelled(cause))
    }

    pub(crate) fn next_ready_wakeup(&self) -> Option<TaskHandle> {
        self.wakeups.iter().find_map(|(id, pending)| {
            let task = self.tasks.get(id)?;
            (task.state == TaskState::Ready && !pending.is_empty()).then_some(TaskHandle {
                owner: task.scope,
                task: *id,
            })
        })
    }

    pub(crate) fn take_wakeup(
        &mut self,
        handle: TaskHandle,
    ) -> Result<Option<TaskWakeup>, SchedulerError> {
        let id = self.resolve_id(handle)?;
        let wakeup = self.wakeups.get_mut(&id).and_then(VecDeque::pop_front);
        if self.wakeups.get(&id).is_some_and(VecDeque::is_empty) {
            self.wakeups.remove(&id);
        }
        Ok(wakeup)
    }

    pub(crate) fn has_unsettled_tasks(&self) -> bool {
        self.tasks.values().any(|task| !task.state.is_terminal())
    }

    pub(crate) fn cancel_unsettled(
        &mut self,
        reason: etas_host::execution::CancellationReason,
    ) -> Result<(), SchedulerError> {
        let handles = self
            .tasks
            .iter()
            .filter(|(_, task)| !task.state.is_terminal())
            .map(|(id, _)| TaskHandle {
                owner: self.tasks[id].scope,
                task: *id,
            })
            .collect::<Vec<_>>();
        for handle in handles {
            self.cancel(handle, reason.clone())?;
        }
        Ok(())
    }

    fn cancel_scope_except(
        &mut self,
        scope: TaskScopeId,
        reason: etas_host::execution::CancellationReason,
        except: TaskId,
    ) -> Result<(), SchedulerError> {
        if !self.groups.contains_key(&scope) {
            return Err(SchedulerError::WrongOwner);
        }
        let scopes = self
            .groups
            .keys()
            .copied()
            .filter(|candidate| self.is_descendant_of(*candidate, scope))
            .collect::<Vec<_>>();
        let handles = scopes
            .iter()
            .flat_map(|scope| self.groups[scope].tasks.iter())
            .filter(|task| **task != except && !self.tasks[task].state.is_terminal())
            .map(|task| TaskHandle {
                owner: self.tasks[task].scope,
                task: *task,
            })
            .collect::<Vec<_>>();
        for handle in handles {
            self.cancel(handle, reason.clone())?;
        }
        Ok(())
    }

    pub(crate) fn primary_failure(&self) -> Option<TaskHandle> {
        self.tasks
            .iter()
            .filter(|(_, task)| matches!(task.outcome, Some(TaskOutcome::Failed(_))))
            .min_by_key(|(id, task)| (task.source_index, **id))
            .map(|(id, _)| TaskHandle {
                owner: self.tasks[id].scope,
                task: *id,
            })
    }

    /// A language body can finish while an abandoned Host supervisor still
    /// owns work. Retain its resource permits and result until that work settles.
    pub(crate) fn finish_host_drain(&mut self, handle: TaskHandle) -> Result<bool, SchedulerError> {
        let id = self.resolve_id(handle)?;
        let task = self.tasks.get_mut(&id).expect("Host owner exists");
        if task.state != TaskState::Waiting(TaskWait::HostDrain)
            || task
                .local
                .host_operations
                .as_ref()
                .is_some_and(|count| count.get() != 0)
        {
            return Ok(false);
        }
        let mut outcome = task
            .pending_outcome
            .take()
            .expect("draining task has its body outcome");
        if matches!(outcome, TaskOutcome::Completed(_) | TaskOutcome::Raised(_))
            && let Some(cause) = task.local.execution.signal()?.cause()?
        {
            outcome = TaskOutcome::Cancelled(cause);
        }
        self.terminal(handle, outcome)?;
        self.apply_group_policy(handle)?;
        Ok(true)
    }

    fn terminal(&mut self, handle: TaskHandle, outcome: TaskOutcome) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let group = self.groups.get_mut(&handle.owner).expect("task scope");
        if group.completion_waiter == Some(id) {
            group.completion_waiter = None;
        }
        self.wakeups.remove(&id);
        self.continuations
            .dispose_origin(TaskIdentity::Scheduled(id));
        let (waiters, was_running) = {
            let task = self.tasks.get_mut(&id).expect("resolved task");
            if task.state.is_terminal() {
                return Err(SchedulerError::InvalidTransition);
            }
            if task
                .local
                .host_operations
                .as_ref()
                .is_some_and(|count| count.get() != 0)
            {
                task.pending_outcome = Some(outcome);
                task.state = TaskState::Waiting(TaskWait::HostDrain);
                task.host_wait = None;
                task.waiting_continuation = None;
                task.machine = EvalMachine::new();
                return Ok(());
            }
            let was_running = task.active_slot;
            task.active_slot = false;
            task.local.execution.finish_body(!matches!(
                outcome,
                TaskOutcome::Raised(_) | TaskOutcome::Failed(_)
            ))?;
            task.state = outcome.state();
            task.outcome = Some(outcome.clone());
            task.waiting_continuation = None;
            task.host_wait = None;
            task.machine = EvalMachine::new();
            (std::mem::take(&mut task.waiters), was_running)
        };
        if was_running {
            let scope = self.tasks[&id].scope;
            self.change_active(scope, false);
        }
        self.resources.release(id);
        let queued = self.resources.waiting.len();
        for _ in 0..queued {
            let waiting = self.resources.waiting.pop_front().unwrap();
            let task = self.tasks.get_mut(&waiting).expect("resource waiter");
            if !matches!(task.state, TaskState::Waiting(TaskWait::Resource(_))) {
                continue;
            }
            if self.resources.acquire(waiting, &task.resource_keys) {
                task.state = TaskState::Ready;
                self.enqueue(waiting);
            } else {
                self.resources.waiting.push_back(waiting);
            }
        }
        for waiter in waiters {
            match waiter {
                TaskWaiter::Task(waiter) => {
                    let task = self.tasks.get_mut(&waiter).expect("registered waiter");
                    if task.state == TaskState::Waiting(TaskWait::Task(id)) {
                        task.state = TaskState::Ready;
                        self.enqueue(waiter);
                        self.wakeups
                            .entry(waiter)
                            .or_default()
                            .push_back(TaskWakeup::Task {
                                completed: handle,
                                outcome: outcome.clone(),
                            });
                    }
                }
            }
        }
        self.record_completion(id);
        self.wake_scope_if_settled(self.tasks[&id].scope)?;
        self.terminal_events.push((handle, outcome.state()));
        Ok(())
    }

    fn resolve(&self, handle: TaskHandle) -> Result<&ScheduledTask, SchedulerError> {
        self.resolve_id(handle)?;
        self.tasks
            .get(&handle.task)
            .ok_or(SchedulerError::UnknownTask)
    }

    fn record_primary_failure(&mut self, scope: TaskScopeId, id: TaskId) {
        let source_index = self.tasks[&id].source_index;
        let replace = self.groups[&scope]
            .primary_failure
            .is_none_or(|primary_id| {
                let primary = &self.tasks[&primary_id];
                (source_index, id) < (primary.source_index, primary_id)
            });
        if replace {
            self.groups
                .get_mut(&scope)
                .expect("task scope must exist")
                .primary_failure = Some(id);
        }
    }

    fn ordered_tasks(&self, group: &group::TaskGroup) -> Vec<TaskId> {
        let mut tasks = group.tasks.iter().copied().collect::<Vec<_>>();
        tasks.sort_by_key(|id| (self.tasks[id].source_index, *id));
        tasks
    }

    fn completed_values(&self, tasks: &[TaskId]) -> Result<Vec<InterpValue>, SchedulerError> {
        tasks
            .iter()
            .map(|task| match self.tasks[task].outcome.as_ref() {
                Some(TaskOutcome::Completed(value)) => Ok(value.clone()),
                _ => Err(SchedulerError::InvalidTransition),
            })
            .collect()
    }

    fn resolve_id(&self, handle: TaskHandle) -> Result<TaskId, SchedulerError> {
        let task = self
            .tasks
            .get(&handle.task)
            .ok_or(SchedulerError::UnknownTask)?;
        if handle.owner != task.scope {
            return Err(SchedulerError::WrongOwner);
        }
        Ok(handle.task)
    }

    fn can_admit(&self, mut scope: TaskScopeId) -> bool {
        loop {
            let group = self.groups.get(&scope).expect("task scope must exist");
            if group.active >= group.concurrency.get() {
                return false;
            }
            let Some(parent) = group.parent else {
                return true;
            };
            scope = parent;
        }
    }

    pub(crate) fn serialize_scope(&mut self, scope: TaskScopeId) {
        self.groups
            .get_mut(&scope)
            .expect("created task scope")
            .serialize_host_actions = true;
    }

    fn scope_serialized(&self, mut scope: TaskScopeId) -> bool {
        loop {
            let group = self.groups.get(&scope).expect("task scope must exist");
            if group.serialize_host_actions {
                return true;
            }
            let Some(parent) = group.parent else {
                return false;
            };
            scope = parent;
        }
    }

    fn serial_task_admission(&self, mut scope: TaskScopeId) -> bool {
        loop {
            let group = &self.groups[&scope];
            if group.serial_task_admission {
                return true;
            }
            let Some(parent) = group.parent else {
                return false;
            };
            scope = parent;
        }
    }

    fn is_descendant_of(&self, mut scope: TaskScopeId, ancestor: TaskScopeId) -> bool {
        loop {
            if scope == ancestor {
                return true;
            }
            let Some(parent) = self.groups[&scope].parent else {
                return false;
            };
            scope = parent;
        }
    }

    fn wake_scope_if_settled(&mut self, scope: TaskScopeId) -> Result<(), SchedulerError> {
        let Some(outcome) = self.scope_outcome(scope)? else {
            return Ok(());
        };
        let waiters = std::mem::take(
            &mut self
                .groups
                .get_mut(&scope)
                .expect("settled task scope must exist")
                .waiters,
        );
        for waiter in waiters {
            let task = self
                .tasks
                .get_mut(&waiter)
                .expect("registered scope waiter");
            if task.state == TaskState::Waiting(TaskWait::Scope(scope)) {
                task.state = TaskState::Ready;
                self.enqueue(waiter);
                self.wakeups
                    .entry(waiter)
                    .or_default()
                    .push_back(TaskWakeup::Scope {
                        scope,
                        outcome: outcome.clone(),
                    });
            }
        }
        Ok(())
    }

    fn change_active(&mut self, mut scope: TaskScopeId, increment: bool) {
        loop {
            let group = self.groups.get_mut(&scope).expect("task scope must exist");
            if increment {
                group.active += 1;
            } else {
                group.active = group
                    .active
                    .checked_sub(1)
                    .expect("active task slot underflow");
            }
            let Some(parent) = group.parent else {
                return;
            };
            scope = parent;
        }
    }
}

fn result_value(name: &str, value: InterpValue) -> InterpValue {
    InterpValue::Variant {
        name: name.to_owned().into(),
        fields: crate::value::SharedFields::new(vec![value]),
    }
}

fn error_payload(perform: &crate::control::PendingPerform) -> InterpValue {
    perform
        .args
        .first()
        .expect("checked Error.raise carries one payload")
        .clone()
}

#[cfg(test)]
mod tests;

mod continuation;
mod task;

use std::{
    collections::{BTreeMap, VecDeque},
    num::NonZeroUsize,
};

pub(crate) use continuation::{ConsumedContinuation, ContinuationTransfer};
pub(crate) use task::{
    ContinuationId, ScheduledTask, TaskHandle, TaskId, TaskIdentity, TaskOutcome, TaskResume,
    TaskScopeId, TaskState, TaskWait, TaskWaiter,
};

pub(crate) const MAX_CONCURRENCY: usize = 65_536;

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
    Task {
        completed: TaskHandle,
        outcome: TaskOutcome,
    },
    Continuation {
        continuation: ContinuationId,
        completed: TaskHandle,
        outcome: TaskOutcome,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum AwaitDecision {
    Ready(TaskOutcome),
    Parked,
}

/// One structured task scope. Tasks remain owned here after their language
/// handles are dropped, and terminal outcomes remain cached for repeat awaits.
pub(crate) struct TaskScheduler {
    owner: TaskScopeId,
    concurrency: NonZeroUsize,
    next_task: u64,
    active: usize,
    tasks: BTreeMap<TaskId, ScheduledTask>,
    ready: VecDeque<TaskId>,
    wakeups: BTreeMap<TaskId, VecDeque<TaskWakeup>>,
    continuations: continuation::ContinuationArena,
}

impl TaskScheduler {
    pub(crate) fn new(owner: TaskScopeId, concurrency: NonZeroUsize) -> Self {
        Self {
            owner,
            concurrency,
            next_task: 0,
            active: 0,
            tasks: BTreeMap::new(),
            ready: VecDeque::new(),
            wakeups: BTreeMap::new(),
            continuations: continuation::ContinuationArena::new(owner),
        }
    }

    pub(crate) fn owner(&self) -> TaskScopeId {
        self.owner
    }

    pub(crate) fn allocate_continuation(
        &mut self,
        origin: TaskIdentity,
        continuation: crate::control::Continuation,
        output: etas_types::TypeId,
    ) -> crate::value::ContinuationTokenValue {
        self.continuations.allocate(origin, continuation, output)
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

    pub(crate) fn spawn(
        &mut self,
        parent: &crate::eval::TaskLocalState,
        machine: EvalMachine,
        program: Option<crate::value::CheckedExecutionProgram>,
        handler_floor: usize,
        source_index: usize,
    ) -> Result<TaskHandle, SchedulerError> {
        let local = parent.child()?;
        let id = TaskId(self.next_task);
        self.next_task = self.next_task.checked_add(1).expect("task id exhausted");
        self.tasks.insert(
            id,
            ScheduledTask::new(machine, local, program, handler_floor, source_index),
        );
        self.ready.push_back(id);
        Ok(TaskHandle {
            owner: self.owner,
            task: id,
        })
    }

    pub(crate) fn state(&self, handle: TaskHandle) -> Result<TaskState, SchedulerError> {
        Ok(self.resolve(handle)?.state)
    }

    pub(crate) fn outcome(
        &self,
        handle: TaskHandle,
    ) -> Result<Option<&TaskOutcome>, SchedulerError> {
        Ok(self.resolve(handle)?.outcome.as_ref())
    }

    /// FIFO admission. Stale queue entries from cancellation are skipped.
    pub(crate) fn dispatch_ready(&mut self) -> Option<TaskHandle> {
        if self.active >= self.concurrency.get() {
            return None;
        }
        while let Some(id) = self.ready.pop_front() {
            let task = self.tasks.get_mut(&id).expect("ready task must exist");
            if task.state != TaskState::Ready {
                continue;
            }
            task.state = TaskState::Running;
            self.active += 1;
            return Some(TaskHandle {
                owner: self.owner,
                task: id,
            });
        }
        None
    }

    pub(crate) fn task_mut(
        &mut self,
        handle: TaskHandle,
    ) -> Result<&mut ScheduledTask, SchedulerError> {
        self.resolve_id(handle)?;
        self.tasks
            .get_mut(&handle.task)
            .ok_or(SchedulerError::UnknownTask)
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
        if handle.owner != self.owner || task.state != TaskState::Running {
            return Err(SchedulerError::InvalidTransition);
        }
        if self.tasks.insert(handle.task, task).is_some() {
            return Err(SchedulerError::InvalidTransition);
        }
        Ok(())
    }

    pub(crate) fn requeue(&mut self, handle: TaskHandle) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let task = self.tasks.get_mut(&id).expect("resolved task");
        if task.state != TaskState::Running {
            return Err(SchedulerError::InvalidTransition);
        }
        task.state = TaskState::Ready;
        self.active -= 1;
        self.ready.push_back(id);
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
        self.active -= 1;
        Ok(AwaitDecision::Parked)
    }

    pub(crate) fn wait_with_continuation(
        &mut self,
        target: TaskHandle,
        continuation: ContinuationId,
    ) -> Result<Option<TaskOutcome>, SchedulerError> {
        let target_id = self.resolve_id(target)?;
        if let Some(outcome) = self.tasks[&target_id].outcome.clone() {
            return Ok(Some(outcome));
        }
        self.tasks
            .get_mut(&target_id)
            .expect("resolved target")
            .waiters
            .push(TaskWaiter::Continuation(continuation));
        Ok(None)
    }

    pub(crate) fn complete(
        &mut self,
        handle: TaskHandle,
        value: InterpValue,
    ) -> Result<(), SchedulerError> {
        self.terminal(handle, TaskOutcome::Completed(value))
    }

    pub(crate) fn fail(
        &mut self,
        handle: TaskHandle,
        failure: RunFailure,
    ) -> Result<(), SchedulerError> {
        self.terminal(handle, TaskOutcome::Failed(failure))
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
        task.local.execution.cancel_source().stop(reason)?;
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

    pub(crate) fn take_wakeup(
        &mut self,
        handle: TaskHandle,
    ) -> Result<Option<TaskWakeup>, SchedulerError> {
        let id = self.resolve_id(handle)?;
        Ok(self.wakeups.get_mut(&id).and_then(VecDeque::pop_front))
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
                owner: self.owner,
                task: *id,
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
                owner: self.owner,
                task: *id,
            })
    }

    fn terminal(&mut self, handle: TaskHandle, outcome: TaskOutcome) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let (waiters, was_running) = {
            let task = self.tasks.get_mut(&id).expect("resolved task");
            if task.state.is_terminal() {
                return Err(SchedulerError::InvalidTransition);
            }
            let was_running = task.state == TaskState::Running;
            task.local
                .execution
                .finish_body(!matches!(outcome, TaskOutcome::Failed(_)))?;
            task.state = outcome.state();
            task.outcome = Some(outcome.clone());
            (std::mem::take(&mut task.waiters), was_running)
        };
        if was_running {
            self.active -= 1;
        }
        for waiter in waiters {
            match waiter {
                TaskWaiter::Task(waiter) => {
                    let task = self.tasks.get_mut(&waiter).expect("registered waiter");
                    if task.state == TaskState::Waiting(TaskWait::Task(id)) {
                        task.state = TaskState::Ready;
                        self.ready.push_back(waiter);
                        self.wakeups
                            .entry(waiter)
                            .or_default()
                            .push_back(TaskWakeup::Task {
                                completed: handle,
                                outcome: outcome.clone(),
                            });
                    }
                }
                TaskWaiter::Continuation(continuation) => {
                    self.wakeups
                        .entry(id)
                        .or_default()
                        .push_back(TaskWakeup::Continuation {
                            continuation,
                            completed: handle,
                            outcome: outcome.clone(),
                        });
                }
            }
        }
        Ok(())
    }

    fn resolve(&self, handle: TaskHandle) -> Result<&ScheduledTask, SchedulerError> {
        self.resolve_id(handle)?;
        self.tasks
            .get(&handle.task)
            .ok_or(SchedulerError::UnknownTask)
    }

    fn resolve_id(&self, handle: TaskHandle) -> Result<TaskId, SchedulerError> {
        if handle.owner != self.owner {
            return Err(SchedulerError::WrongOwner);
        }
        if !self.tasks.contains_key(&handle.task) {
            return Err(SchedulerError::UnknownTask);
        }
        Ok(handle.task)
    }
}

#[cfg(test)]
mod tests;

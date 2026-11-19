use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TaskCompletion {
    pub(crate) task: TaskId,
    pub(crate) order: u64,
}

pub(crate) enum CompletionDecision {
    Ready(TaskCompletion),
    Parked,
}

impl TaskScheduler {
    /// Queue observation and waiter registration are one scheduler operation.
    pub(crate) fn await_completion(
        &mut self,
        waiter: TaskHandle,
        scope: TaskScopeId,
    ) -> Result<CompletionDecision, SchedulerError> {
        let id = self.resolve_id(waiter)?;
        if waiter.owner != scope
            || self.groups.get(&scope).and_then(|group| group.owner) != Some(id)
        {
            return Err(SchedulerError::WrongOwner);
        }
        if self.tasks[&id].state != TaskState::Running {
            return Err(SchedulerError::InvalidTransition);
        }
        let group = self.groups.get_mut(&scope).expect("completion scope");
        if let Some(event) = group.completions.pop_front() {
            return Ok(CompletionDecision::Ready(event));
        }
        assert!(
            group.completion_waiter.replace(id).is_none(),
            "one completion consumer per scope"
        );
        let task = self.tasks.get_mut(&id).expect("completion waiter");
        task.state = TaskState::Waiting(TaskWait::Completion(scope));
        if task.active_slot {
            task.active_slot = false;
            self.change_active(scope, false);
        }
        Ok(CompletionDecision::Parked)
    }

    pub(super) fn record_completion(&mut self, id: TaskId) {
        let task = &self.tasks[&id];
        if task.start.is_none() {
            return;
        }
        let scope = task.scope;
        let group = self.groups.get_mut(&scope).expect("completed task scope");
        let event = TaskCompletion {
            task: id,
            order: group.next_completion_order,
        };
        group.next_completion_order = group
            .next_completion_order
            .checked_add(1)
            .expect("task completion order exhausted");
        group.completions.push_back(event);
        if let Some(waiter) = group.completion_waiter.take() {
            let event = group.completions.pop_front().expect("published completion");
            let task = self.tasks.get_mut(&waiter).expect("completion consumer");
            assert_eq!(task.state, TaskState::Waiting(TaskWait::Completion(scope)));
            task.state = TaskState::Ready;
            self.wakeups
                .entry(waiter)
                .or_default()
                .push_back(TaskWakeup::Completion { scope, event });
            self.enqueue(waiter);
        }
    }
}

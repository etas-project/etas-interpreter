use super::*;
use crate::eval::machine::snapshot::{CaptureContext, RestoreContext};

/// The scheduling half of a checkpoint. Machine/local payloads are captured and
/// restored by the evaluator, which owns checked programs and frame graphs.
#[derive(Clone, Debug)]
pub(crate) struct TaskSchedulerMetadataSnapshot {
    pub(crate) next_scope: u64,
    pub(crate) next_task: u64,
    pub(crate) groups: BTreeMap<TaskScopeId, TaskScopeSnapshot>,
    pub(crate) ready: VecDeque<TaskId>,
    pub(crate) wakeups: BTreeMap<TaskId, VecDeque<TaskWakeupSnapshot>>,
    pub(crate) continuations: ContinuationArenaSnapshot,
    pub(crate) resources: ResourceGuardsSnapshot,
}

// Outcomes are already stored by the completed task/scope. Save their stable
// reference so wakeups share the restored outcome and its captured values.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum TaskWakeupSnapshot {
    Task { completed: TaskHandle },
    Scope { scope: TaskScopeId },
}

impl TaskScheduler {
    pub(crate) fn capture_metadata(
        &self,
        context: &mut CaptureContext,
    ) -> Result<TaskSchedulerMetadataSnapshot, String> {
        assert!(
            self.checkpoint_barrier,
            "scheduler capture requires a checkpoint barrier"
        );
        assert!(
            self.cancellation_events.is_empty(),
            "cancellation events must be committed before capture"
        );
        let snapshot = TaskSchedulerMetadataSnapshot {
            next_scope: self.next_scope,
            next_task: self.next_task,
            groups: self
                .groups
                .iter()
                .map(|(id, group)| (*id, TaskScopeSnapshot(group.clone())))
                .collect(),
            ready: self.ready.clone(),
            wakeups: self
                .wakeups
                .iter()
                .map(|(id, wakeups)| {
                    (
                        *id,
                        wakeups
                            .iter()
                            .map(|wakeup| match wakeup {
                                TaskWakeup::Task { completed, .. } => TaskWakeupSnapshot::Task {
                                    completed: *completed,
                                },
                                TaskWakeup::Scope { scope, .. } => {
                                    TaskWakeupSnapshot::Scope { scope: *scope }
                                }
                            })
                            .collect(),
                    )
                })
                .collect(),
            continuations: self.continuations.capture(context)?,
            resources: self.resources.capture(),
        };
        snapshot.validate(&self.tasks)?;
        Ok(snapshot)
    }

    pub(crate) fn restore_metadata(
        snapshot: TaskSchedulerMetadataSnapshot,
        tasks: BTreeMap<TaskId, ScheduledTask>,
        context: &mut RestoreContext,
    ) -> Result<Self, String> {
        snapshot.validate(&tasks)?;
        let mut scheduler = Self {
            next_scope: snapshot.next_scope,
            next_task: snapshot.next_task,
            groups: snapshot
                .groups
                .into_iter()
                .map(|(id, group)| (id, group.0))
                .collect(),
            tasks,
            ready: snapshot.ready,
            wakeups: BTreeMap::new(),
            cancellation_events: Vec::new(),
            continuations: continuation::ContinuationArena::restore(
                snapshot.continuations,
                context,
            )?,
            resources: resource::ResourceGuards::restore(snapshot.resources),
            checkpoint_barrier: false,
        };
        for (id, wakeups) in snapshot.wakeups {
            let restored = wakeups
                .into_iter()
                .map(|wakeup| {
                    Ok(match wakeup {
                        TaskWakeupSnapshot::Task { completed } => TaskWakeup::Task {
                            completed,
                            outcome: scheduler
                                .outcome(completed)
                                .map_err(|error| error.to_string())?
                                .cloned()
                                .ok_or("checkpoint wakeup refers to an unfinished task")?,
                        },
                        TaskWakeupSnapshot::Scope { scope } => TaskWakeup::Scope {
                            scope,
                            outcome: scheduler
                                .scope_outcome(scope)
                                .map_err(|error| error.to_string())?
                                .ok_or("checkpoint wakeup refers to an unfinished scope")?,
                        },
                    })
                })
                .collect::<Result<_, String>>()?;
            scheduler.wakeups.insert(id, restored);
        }
        Ok(scheduler)
    }
}

impl TaskSchedulerMetadataSnapshot {
    fn validate(&self, tasks: &BTreeMap<TaskId, ScheduledTask>) -> Result<(), String> {
        self.continuations.validate()?;
        let mut active: BTreeMap<_, usize> = self.groups.keys().map(|id| (*id, 0)).collect();
        for (id, saved) in &self.groups {
            let group = &saved.0;
            if id.0 >= self.next_scope
                || group
                    .parent
                    .is_some_and(|parent| parent >= *id || !self.groups.contains_key(&parent))
            {
                return Err("checkpoint scope ancestry/counter is inconsistent".into());
            }
            if group
                .tasks
                .iter()
                .any(|task| tasks.get(task).is_none_or(|task| task.scope != *id))
                || [group.owner, group.primary_failure, group.winner]
                    .into_iter()
                    .flatten()
                    .any(|task| !group.tasks.contains(&task))
                || group.waiters.iter().any(|task| !tasks.contains_key(task))
            {
                return Err("checkpoint scope refers to a task with a different owner".into());
            }
        }
        for (id, task) in tasks {
            let group = self
                .groups
                .get(&task.scope)
                .ok_or("checkpoint task has no owning scope")?;
            if id.0 >= self.next_task || !group.0.tasks.contains(id) {
                return Err("checkpoint task identity/counter is inconsistent".into());
            }
            if matches!(
                task.state,
                TaskState::Running | TaskState::Waiting(TaskWait::Cpu(_) | TaskWait::Host(_))
            ) {
                return Err(
                    "checkpoint task is still running outside the coordinator barrier".into(),
                );
            }
            if task.active_slot {
                let mut scope = task.scope;
                loop {
                    *active.get_mut(&scope).expect("validated scope ancestry") += 1;
                    let Some(parent) = self.groups[&scope].0.parent else {
                        break;
                    };
                    scope = parent;
                }
            }
        }
        if self
            .groups
            .iter()
            .any(|(id, group)| group.0.active != active[id])
        {
            return Err("checkpoint scope active slots differ from admitted tasks".into());
        }
        if self
            .ready
            .iter()
            .chain(self.wakeups.keys())
            .chain(self.resources.waiting.iter())
            .any(|id| !tasks.contains_key(id))
        {
            return Err("checkpoint queue refers to an unknown task".into());
        }
        for (id, keys) in &self.resources.holders {
            if tasks
                .get(id)
                .is_none_or(|task| task.state.is_terminal() || task.resource_keys != *keys)
            {
                return Err("checkpoint resource permits differ from their admitted task".into());
            }
        }
        for entry in self.continuations.entries.values() {
            if !self.groups.contains_key(&entry.owner)
                || matches!(entry.origin,
                TaskIdentity::Scheduled(id) if !tasks.contains_key(&id))
            {
                return Err("checkpoint continuation has no owning task/scope".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{root_local, spawn};
    use super::*;

    #[test]
    fn metadata_roundtrip_restores_nested_scope_waiters_fifo_and_resource_permits() {
        let root_scope = TaskScopeId(7);
        let root = root_local(root_scope);
        let mut scheduler = TaskScheduler::new(root_scope, NonZeroUsize::new(2).unwrap());
        let parent = spawn(&mut scheduler, &root, 0);
        scheduler.set_scope_owner(root_scope, parent).unwrap();
        let scope = scheduler.create_scope_with_policy(
            root_scope,
            NonZeroUsize::new(2).unwrap(),
            TaskGroupPolicy::JoinArray,
        );
        let mut tasks = Vec::new();
        for (index, key) in ["a", "a", "b"].into_iter().enumerate() {
            let handle = scheduler
                .spawn_in_scope(&root, scope, EvalMachine::new(), None, 0, index)
                .unwrap();
            scheduler
                .attach_resources(
                    handle,
                    vec![RuntimeResourceKey {
                        effect: EffectIdentity::Memory,
                        selector: FrozenSelector {
                            region: "test".into(),
                            path: Vec::new(),
                            key: Some(key.into()),
                        },
                    }],
                )
                .unwrap();
            tasks.push(handle);
        }
        assert_eq!(scheduler.dispatch_ready(), Some(parent));
        assert!(matches!(
            scheduler.await_scope(parent, scope).unwrap(),
            ScopeDecision::Parked
        ));
        assert_eq!(scheduler.dispatch_ready(), Some(tasks[0]));
        scheduler.requeue(tasks[0]).unwrap();
        assert_eq!(scheduler.dispatch_ready(), Some(tasks[2]));
        scheduler.requeue(tasks[2]).unwrap();
        assert!(matches!(
            scheduler.state(tasks[1]).unwrap(),
            TaskState::Waiting(TaskWait::Resource(_))
        ));
        scheduler.take_resource_events();
        scheduler.begin_checkpoint_barrier();
        let metadata = scheduler
            .capture_metadata(&mut CaptureContext::default())
            .unwrap();
        let metadata =
            crate::api::codec::checkpoint::scheduler::roundtrip_scheduler_metadata(&metadata);
        let payloads = std::mem::take(&mut scheduler.tasks);
        let mut restored =
            TaskScheduler::restore_metadata(metadata, payloads, &mut RestoreContext::default())
                .unwrap();
        assert_eq!(
            restored.state(parent).unwrap(),
            TaskState::Waiting(TaskWait::Scope(scope))
        );
        assert_eq!(restored.dispatch_ready(), Some(tasks[0]));
        restored.complete(tasks[0], InterpValue::i32(42)).unwrap();
        assert_eq!(restored.dispatch_ready(), Some(tasks[2]));
        restored.complete(tasks[2], InterpValue::i32(43)).unwrap();
        assert_eq!(restored.dispatch_ready(), Some(tasks[1]));
        restored.complete(tasks[1], InterpValue::i32(44)).unwrap();
        let acquired = restored
            .take_resource_events()
            .into_iter()
            .filter(|event| matches!(event, ResourceEvent::Acquired(..)))
            .count();
        assert_eq!(
            acquired, 1,
            "only the blocked task takes a new permit after restore"
        );
        restored.begin_checkpoint_barrier();
        let metadata = restored
            .capture_metadata(&mut CaptureContext::default())
            .unwrap();
        let metadata =
            crate::api::codec::checkpoint::scheduler::roundtrip_scheduler_metadata(&metadata);
        let payloads = std::mem::take(&mut restored.tasks);
        let mut restored =
            TaskScheduler::restore_metadata(metadata, payloads, &mut RestoreContext::default())
                .unwrap();
        assert_eq!(restored.dispatch_ready(), Some(parent));
        assert!(
            matches!(restored.take_wakeup(parent).unwrap(), Some(TaskWakeup::Scope {
            scope: resumed_scope, outcome: TaskOutcome::Completed(value),
        }) if resumed_scope == scope && value == InterpValue::Array(vec![
            InterpValue::i32(42), InterpValue::i32(44), InterpValue::i32(43)
        ].into()))
        );
        assert_eq!(
            restored.create_scope(root_scope, NonZeroUsize::new(1).unwrap()),
            TaskScopeId(scope.0 + 1)
        );
    }
}

use super::*;

/// A source policy owns ordinary value state; executable identities stay tied
/// to the checked program that packed the existential implementation.
#[derive(Clone, Debug)]
pub(crate) struct SourceSchedulingPolicy {
    pub(crate) program: crate::value::CheckedExecutionProgram,
    pub(crate) instance: Box<crate::value::ExistentialValue>,
    pub(crate) entry: etas_hir::HirItemId,
    pub(crate) span: etas_core::Span,
}

#[derive(Clone, Debug)]
pub(crate) struct SourcePolicySnapshot {
    pub(crate) program: crate::orchestration::ExecutionProgramSnapshot,
    pub(crate) instance: crate::orchestration::ValueSnapshot,
    pub(crate) entry: etas_hir::HirItemId,
    pub(crate) span: etas_core::Span,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ReadyTaskSnapshot {
    pub(crate) handle: TaskHandle,
    pub(crate) enqueue_order: u64,
}

pub(crate) struct ReadyDispatchSnapshot {
    pub(crate) policy_owner: Option<TaskScopeId>,
    pub(crate) entries: Vec<ReadyTaskSnapshot>,
}

impl TaskScheduler {
    pub(crate) fn install_source_policy(
        &mut self,
        scope: TaskScopeId,
        policy: SourceSchedulingPolicy,
    ) {
        assert!(self.groups.contains_key(&scope), "policy scope must exist");
        assert!(
            self.policies.insert(scope, policy).is_none(),
            "scope policy is installed once"
        );
    }

    pub(crate) fn has_source_policies(&self) -> bool {
        !self.policies.is_empty()
    }

    fn policy_owner(&self, mut scope: TaskScopeId) -> Option<TaskScopeId> {
        loop {
            if self.policies.contains_key(&scope) {
                return Some(scope);
            }
            scope = self.groups[&scope].parent?;
        }
    }

    pub(crate) fn current_source_policy(
        &self,
        scope: TaskScopeId,
    ) -> Option<&SourceSchedulingPolicy> {
        self.policy_owner(scope)
            .and_then(|owner| self.policies.get(&owner))
    }

    pub(crate) fn take_source_policy(&mut self, owner: TaskScopeId) -> SourceSchedulingPolicy {
        self.policies
            .remove(&owner)
            .expect("selected source policy exists")
    }

    pub(crate) fn restore_source_policy(
        &mut self,
        owner: TaskScopeId,
        policy: SourceSchedulingPolicy,
    ) {
        assert!(
            self.policies.insert(owner, policy).is_none(),
            "selection has exclusive policy access"
        );
    }

    /// Runtime admission determines which entries can run. Source chooses among
    /// those entries; it cannot acquire permits or alter live queue ownership.
    pub(crate) fn ready_snapshot(&mut self) -> Option<ReadyDispatchSnapshot> {
        if self.checkpoint_barrier {
            return None;
        }
        let mut candidates = Vec::new();
        let queued = self.ready.len();
        for _ in 0..queued {
            let id = self.ready.pop_front().expect("bounded ready scan");
            let task = &self.tasks[&id];
            if task.state != TaskState::Ready {
                self.enqueue_orders.remove(&id);
                continue;
            }
            if !task.active_slot && !self.can_admit(task.scope) {
                self.ready.push_back(id);
                continue;
            }
            if !self.resources.available(id, &task.resource_keys) {
                let guard = resource::ResourceGuardId(id.0);
                self.tasks.get_mut(&id).unwrap().state =
                    TaskState::Waiting(TaskWait::Resource(guard));
                self.resources.waiting.push_back(id);
                self.resources
                    .events
                    .push(resource::ResourceEvent::Waiting(id, guard));
                self.enqueue_orders.remove(&id);
                continue;
            }
            let handle = TaskHandle {
                owner: task.scope,
                task: id,
            };
            candidates.push(ReadyTaskSnapshot {
                handle,
                enqueue_order: self.enqueue_orders[&id],
            });
            self.ready.push_back(id);
        }
        let owner = self.policy_owner(candidates.first()?.handle.owner);
        candidates.retain(|entry| self.policy_owner(entry.handle.owner) == owner);
        if owner.is_some() {
            candidates.sort_by_key(|entry| entry.enqueue_order);
        }
        Some(ReadyDispatchSnapshot {
            policy_owner: owner,
            entries: candidates,
        })
    }

    pub(crate) fn dispatch_selected(
        &mut self,
        handle: TaskHandle,
    ) -> Result<Option<TaskHandle>, SchedulerError> {
        let task = self.resolve(handle)?;
        if task.state != TaskState::Ready || (!task.active_slot && !self.can_admit(task.scope)) {
            return Ok(None);
        }
        let active_slot = task.active_slot;
        let scope = task.scope;
        let keys = task.resource_keys.clone();
        if !self.resources.acquire(handle.task, &keys) {
            return Ok(None);
        }
        let position = self
            .ready
            .iter()
            .position(|id| *id == handle.task)
            .ok_or(SchedulerError::InvalidTransition)?;
        self.ready.remove(position);
        self.enqueue_orders.remove(&handle.task);
        let task = self.tasks.get_mut(&handle.task).unwrap();
        task.state = TaskState::Running;
        if !active_slot {
            task.active_slot = true;
            self.change_active(scope, true);
        }
        Ok(Some(handle))
    }

    pub(super) fn enqueue(&mut self, id: TaskId) {
        let scope = self.tasks[&id].scope;
        let owner = self.policy_owner(scope).unwrap_or(scope);
        let next = self.next_enqueue_order.entry(owner).or_default();
        let order = *next;
        *next = order.checked_add(1).expect("enqueue order exhausted");
        self.enqueue_orders.insert(id, order);
        self.ready.push_back(id);
    }
}

impl TaskScheduler {
    pub(crate) fn capture_policies(
        &self,
        context: &mut crate::eval::machine::snapshot::CaptureContext,
    ) -> Result<BTreeMap<TaskScopeId, SourcePolicySnapshot>, String> {
        self.policies
            .iter()
            .map(|(scope, policy)| {
                Ok((
                    *scope,
                    SourcePolicySnapshot {
                        program: context.execution_identity(&policy.program)?,
                        instance: context
                            .value(&InterpValue::Existential(policy.instance.clone()))?,
                        entry: policy.entry,
                        span: policy.span,
                    },
                ))
            })
            .collect()
    }
}

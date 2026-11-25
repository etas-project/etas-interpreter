use super::*;
use crate::orchestration::WorkflowEvent;
use crate::value::{AgentReference, CheckedExecutionProgram};

pub(super) struct AgentInstances {
    run: u64,
    next_instance: u64,
    events: Vec<WorkflowEvent>,
    instances: BTreeMap<u64, AgentInstance>,
    waiting_owners: BTreeMap<TaskId, Vec<u64>>,
    autonomous: std::collections::BTreeSet<TaskId>,
}

impl Default for AgentInstances {
    fn default() -> Self {
        static NEXT_RUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            run: NEXT_RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            next_instance: 0,
            events: Vec::new(),
            instances: BTreeMap::new(),
            waiting_owners: BTreeMap::new(),
            autonomous: Default::default(),
        }
    }
}

struct AgentInstance {
    reference: AgentReference,
    owner: TaskScopeId,
    lifetime: std::rc::Rc<crate::control::AgentLifetime>,
    owner_budget: etas_host::ExecutionBudget,
    owner_authorities: Vec<etas_host::AuthorityContext>,
    model_restrictions: Vec<crate::eval::model_restrictions::AgentModelRestrictions>,
    program: CheckedExecutionProgram,
    committed: Option<InterpValue>,
    version: u64,
    writer: Option<TaskId>,
    waiting: VecDeque<TaskId>,
}

/// A turn belongs to one request, never to an AgentReference or lexical capture.
pub(crate) struct AgentTurn {
    pub(crate) reference: AgentReference,
    pub(crate) updating: bool,
    pub(crate) state: Option<InterpValue>,
    pub(crate) tool_snapshots: Vec<std::rc::Rc<crate::value::MemberToolSnapshot>>,
}

impl Drop for AgentInstances {
    fn drop(&mut self) {
        for instance in self.instances.values() {
            let _ = instance.lifetime.close();
        }
    }
}

impl TaskScheduler {
    pub(crate) fn begin_record_construction(
        &self,
        parent: &crate::eval::TaskLocalState,
    ) -> std::rc::Rc<crate::control::RecordConstruction> {
        let (owner, execution) = if let Some(initialization) = parent.agent_initialization_owner() {
            (initialization.owner, initialization.execution.clone())
        } else if let Some(record) = parent
            .pending_record_constructions()
            .into_iter()
            .min_by_key(|record| record.id)
        {
            (record.owner, record.execution())
        } else {
            let owner = parent.host_task_scope();
            let execution = self.groups[&owner]
                .owner
                .and_then(|owner| self.tasks.get(&owner))
                .map_or_else(
                    || parent.host_execution(),
                    |owner| owner.local.execution.clone(),
                );
            (owner, execution)
        };
        let guard = crate::control::RecordConstruction::new(parent.record_task_identity(), owner);
        guard.bind_execution(execution);
        guard
    }

    pub(crate) fn restore_record_contexts(
        &mut self,
        guards: &[std::rc::Rc<crate::control::RecordConstruction>],
        contexts: &[std::rc::Weak<crate::control::RecordContext>],
        root: &crate::eval::TaskLocalState,
    ) {
        for guard in guards {
            let execution = self.groups[&guard.owner]
                .owner
                .and_then(|owner| self.tasks.get(&owner))
                .map_or_else(
                    || root.execution.clone(),
                    |owner| owner.local.execution.clone(),
                );
            guard.bind_execution(execution);
        }
        let ids: Vec<_> = self.tasks.keys().copied().collect();
        for id in ids {
            let parent = self.tasks[&id].local.parent.expect("restored task parent");
            let inherited = match parent {
                TaskIdentity::Root => root.pending_record_constructions(),
                TaskIdentity::Scheduled(parent) => {
                    self.tasks[&parent].local.pending_record_constructions()
                }
            };
            let local = &mut self.tasks.get_mut(&id).expect("restored task").local;
            local.record_constructions = guards.iter().map(std::rc::Rc::downgrade).collect();
            local.inherited_record_constructions =
                inherited.iter().map(std::rc::Rc::downgrade).collect();
            local.borrowed_record_contexts = contexts
                .iter()
                .filter(|weak| {
                    weak.upgrade()
                        .is_some_and(|context| context.owner == TaskIdentity::Scheduled(id))
                })
                .cloned()
                .collect();
        }
    }

    pub(crate) fn close_agent_scope(&mut self, scope: TaskScopeId) -> Result<(), SchedulerError> {
        for instance in self
            .agents
            .instances
            .values()
            .filter(|instance| instance.owner == scope)
        {
            instance.lifetime.close()?;
        }
        Ok(())
    }

    pub(super) fn wait_for_owned_agents(
        &mut self,
        handle: TaskHandle,
        outcome: &TaskOutcome,
    ) -> Result<bool, SchedulerError> {
        if self.groups[&handle.owner].owner != Some(handle.task) {
            return Ok(false);
        }
        self.close_agent_scope(handle.owner)?;
        let instances: Vec<_> = self
            .agents
            .instances
            .values()
            .filter(|instance| instance.owner == handle.owner)
            .map(|instance| instance.reference.id)
            .collect();
        let requests: Vec<_> = self
            .tasks
            .iter()
            .filter_map(|(id, task)| {
                (!task.state.is_terminal()
                    && task
                        .local
                        .agent_binding
                        .is_some_and(|reference| instances.contains(&reference.id)))
                .then_some(TaskHandle {
                    owner: task.scope,
                    task: *id,
                })
            })
            .collect();
        for request in &requests {
            self.cancel(
                *request,
                etas_host::execution::CancellationReason::OwnerDropped,
            )?;
        }
        if requests
            .iter()
            .all(|request| self.tasks[&request.task].state.is_terminal())
        {
            return Ok(false);
        }
        self.agents.waiting_owners.insert(handle.task, instances);
        let task = self.tasks.get_mut(&handle.task).expect("Agent owner");
        task.pending_outcome = Some(outcome.clone());
        task.state = TaskState::Waiting(TaskWait::ResourceChildren);
        task.machine = EvalMachine::new();
        task.waiting_continuation = None;
        let active = std::mem::replace(&mut task.active_slot, false);
        if active {
            self.change_active(handle.owner, false);
        }
        self.release_task_resources(handle.task);
        Ok(true)
    }

    pub(crate) fn reap_closed_agents(&mut self) -> Result<(), SchedulerError> {
        let requests: Vec<_> = self
            .tasks
            .iter()
            .filter_map(|(id, task)| {
                (!task.state.is_terminal()
                    && task.local.agent_binding.is_some_and(|reference| {
                        self.agents
                            .instances
                            .get(&reference.id)
                            .is_some_and(|instance| instance.lifetime.is_closed())
                    }))
                .then_some(TaskHandle {
                    owner: task.scope,
                    task: *id,
                })
            })
            .collect();
        for request in requests {
            self.cancel(
                request,
                etas_host::execution::CancellationReason::OwnerDropped,
            )?;
        }
        let mut cancelled = Vec::new();
        for (id, task) in &self.tasks {
            if !task.state.is_terminal()
                && task.state != TaskState::Running
                && task.local.execution.signal()?.cause()?.is_some()
            {
                cancelled.push(TaskHandle {
                    owner: task.scope,
                    task: *id,
                });
            }
        }
        for task in cancelled {
            self.cancel(task, etas_host::execution::CancellationReason::OwnerDropped)?;
        }
        let owners: Vec<_> = self
            .agents
            .waiting_owners
            .iter()
            .filter_map(|(owner, instances)| {
                self.tasks
                    .values()
                    .all(|task| {
                        task.state.is_terminal()
                            || !task
                                .local
                                .agent_binding
                                .is_some_and(|reference| instances.contains(&reference.id))
                    })
                    .then_some(*owner)
            })
            .collect();
        for owner in owners {
            self.agents.waiting_owners.remove(&owner);
            let task = self.tasks.get_mut(&owner).expect("waiting Agent owner");
            let handle = TaskHandle {
                owner: task.scope,
                task: owner,
            };
            let outcome = task.pending_outcome.take().expect("Agent owner outcome");
            self.terminal(handle, outcome)?;
        }
        self.agents.instances.retain(|id, instance| {
            !instance.lifetime.is_closed()
                || self.tasks.values().any(|task| {
                    !task.state.is_terminal()
                        && task
                            .local
                            .agent_binding
                            .is_some_and(|reference| reference.id == *id)
                })
        });
        Ok(())
    }

    pub(crate) fn begin_agent_construction(
        &mut self,
        parent: &crate::eval::TaskLocalState,
        program: CheckedExecutionProgram,
        ty: etas_types::TypeId,
    ) -> Result<std::rc::Rc<crate::control::AgentInitialization>, SchedulerError> {
        let owner_budget = match parent.model_policy.budget.clone() {
            Some(limits) => parent.host_budget().with_limits(limits)?,
            None => parent.host_budget(),
        };
        owner_budget.check_time()?;
        let mut model_restrictions = parent.model_restrictions();
        let restriction = crate::eval::model_restrictions::AgentModelRestrictions::capture(
            &parent.model_policy,
            parent.host_context().authority.policy,
        );
        if !model_restrictions.contains(&restriction) {
            model_restrictions.push(restriction);
        }
        let initialization_owner = parent.agent_initialization_owner();
        let record_owner = parent
            .pending_record_constructions()
            .into_iter()
            .min_by_key(|record| record.id);
        let owner = initialization_owner.as_ref().map_or_else(
            || {
                record_owner
                    .as_ref()
                    .map_or(parent.host_task_scope(), |record| record.owner)
            },
            |owner| owner.owner,
        );
        let owner_execution = record_owner
            .as_ref()
            .map(|record| record.execution())
            .unwrap_or_else(|| {
                self.groups[&owner]
                    .owner
                    .and_then(|owner| self.tasks.get(&owner))
                    .map_or_else(
                        || parent.host_execution(),
                        |owner| owner.local.execution.clone(),
                    )
            });
        let execution = initialization_owner
            .as_ref()
            .map_or_else(|| owner_execution.child(), |owner| owner.execution.child())?;
        let lifetime =
            crate::control::AgentLifetime::new(execution, owner, initialization_owner.as_ref());
        for guard in parent.pending_record_constructions() {
            guard.register(&lifetime);
        }
        let reference = AgentReference {
            run: self.agents.run,
            id: self.agents.next_instance,
            ty,
            program: std::sync::Arc::as_ptr(&program.checked) as usize,
        };
        self.agents.next_instance = self
            .agents
            .next_instance
            .checked_add(1)
            .expect("agent id exhausted");
        self.agents.instances.insert(
            reference.id,
            AgentInstance {
                reference,
                owner,
                lifetime: lifetime.clone(),
                owner_budget,
                model_restrictions,
                owner_authorities: std::iter::once(
                    parent.host_context().authority.with_approvals(),
                )
                .chain(parent.authority_constraints().into_iter())
                .collect(),
                program,
                committed: None,
                version: 0,
                writer: None,
                waiting: VecDeque::new(),
            },
        );
        Ok(crate::control::AgentInitialization::new(
            reference,
            parent.record_task_identity(),
            lifetime,
        ))
    }

    pub(crate) fn publish_agent_construction(
        &mut self,
        initialization: &crate::control::AgentInitialization,
        state: InterpValue,
    ) -> Result<AgentReference, SchedulerError> {
        let reference = initialization.reference;
        let instance = self
            .agents
            .instances
            .get_mut(&reference.id)
            .ok_or(SchedulerError::WrongOwner)?;
        if instance.reference != reference
            || !initialization.is_pending()
            || instance.committed.is_some()
        {
            return Err(SchedulerError::WrongOwner);
        }
        instance.committed = Some(state);
        initialization.publish();
        self.agents.events.push(WorkflowEvent::AgentCreated {
            instance: reference.id,
            scope: instance.owner.0,
        });
        Ok(reference)
    }

    #[cfg(test)]
    pub(crate) fn construct_agent(
        &mut self,
        parent: &crate::eval::TaskLocalState,
        program: CheckedExecutionProgram,
        ty: etas_types::TypeId,
        state: InterpValue,
    ) -> Result<AgentReference, SchedulerError> {
        let initialization = self.begin_agent_construction(parent, program, ty)?;
        self.publish_agent_construction(&initialization, state)
    }

    pub(crate) fn hold_agent_request(
        &mut self,
        handle: TaskHandle,
        reference: AgentReference,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let task = self.tasks.get_mut(&id).expect("bound request");
        assert_eq!(task.state, TaskState::Ready);
        assert_eq!(task.local.agent_binding, Some(reference));
        task.state = TaskState::Waiting(TaskWait::AgentAdmission(reference.id));
        self.ready.retain(|ready| *ready != id);
        self.enqueue_orders.remove(&id);
        Ok(())
    }

    pub(crate) fn bind_agent_request_context(
        &mut self,
        handle: TaskHandle,
        reference: AgentReference,
        program: &CheckedExecutionProgram,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let instance = self
            .agents
            .instances
            .get_mut(&reference.id)
            .ok_or(SchedulerError::WrongOwner)?;
        if instance.reference != reference
            || instance.reference.ty != reference.ty
            || instance.reference.program != reference.program
            || !std::sync::Arc::ptr_eq(&instance.program.checked, &program.checked)
            || !self.groups.contains_key(&instance.owner)
            || instance.committed.is_none()
            || instance.lifetime.is_closed()
            || instance.lifetime.execution.termination()?.is_some()
            || instance.lifetime.execution.signal()?.cause()?.is_some()
        {
            return Err(SchedulerError::WrongOwner);
        }
        let request = &mut self.tasks.get_mut(&id).expect("request task").local;
        if let Some(bound) = request.agent_binding {
            return if bound == reference {
                Ok(())
            } else {
                Err(SchedulerError::WrongOwner)
            };
        }
        request.agent_request_root = Some(request.identity);
        let caller_restrictions = crate::eval::model_restrictions::AgentModelRestrictions::capture(
            &request.model_policy,
            request.host_context.authority.policy.clone(),
        );
        for restriction in
            std::iter::once(&caller_restrictions).chain(instance.model_restrictions.iter())
        {
            if !request.model_restrictions.contains(restriction) {
                request.model_restrictions.push(restriction.clone());
            }
        }
        let caller_budget = match request.model_policy.budget.clone() {
            Some(limits) => request.host_budget().with_limits(limits)?,
            None => request.host_budget(),
        };
        request.host_context.budget = caller_budget.conjoin(&instance.owner_budget)?;
        for restriction in &instance.owner_authorities {
            if !request.authority_constraints.contains(restriction) {
                request.authority_constraints.push(restriction.clone());
            }
        }
        request.agent_binding = Some(reference);
        Ok(())
    }

    pub(crate) fn admit_agent_request(
        &mut self,
        handle: TaskHandle,
        reference: AgentReference,
        updating: bool,
        program: &CheckedExecutionProgram,
    ) -> Result<(), SchedulerError> {
        self.bind_agent_request_context(handle, reference, program)?;
        let id = self.resolve_id(handle)?;
        let instance = self
            .agents
            .instances
            .get_mut(&reference.id)
            .expect("bound Agent instance");
        self.agents.events.push(WorkflowEvent::AgentRequestQueued {
            instance: reference.id,
            request: id.0,
            updating,
        });
        let state = if updating && instance.writer.is_some() {
            instance.waiting.push_back(id);
            self.tasks.get_mut(&id).expect("request task").state =
                TaskState::Waiting(TaskWait::Agent(reference.id));
            None
        } else {
            if updating {
                instance.writer = Some(id);
            }
            self.agents
                .events
                .push(WorkflowEvent::AgentRequestAdmitted {
                    instance: reference.id,
                    request: id.0,
                    updating,
                    version: instance.version,
                });
            Some(
                instance
                    .committed
                    .as_ref()
                    .expect("published Agent state")
                    .clone(),
            )
        };
        let admitted = state.is_some();
        let request = self.tasks.get_mut(&id).expect("request task");
        request.local.agent_turn = Some(AgentTurn {
            reference,
            updating,
            state,
            tool_snapshots: Vec::new(),
        });
        if admitted && request.state == TaskState::Waiting(TaskWait::AgentAdmission(reference.id)) {
            request.state = TaskState::Ready;
            self.enqueue(id);
        }
        Ok(())
    }

    /// Publish before task waiters can receive a successful reply. An Err value
    /// is ordinary completion; raised actions and cancellation discard state.
    pub(super) fn finish_agent_request(&mut self, id: TaskId, completed: bool) {
        let Some(turn) = self
            .tasks
            .get_mut(&id)
            .expect("request task")
            .local
            .agent_turn
            .take()
        else {
            return;
        };
        let instance = self
            .agents
            .instances
            .get_mut(&turn.reference.id)
            .expect("live instance");
        if !turn.updating {
            self.agents.events.push(WorkflowEvent::AgentRequestSettled {
                instance: turn.reference.id,
                request: id.0,
                committed: false,
                version: instance.version,
            });
            return;
        }
        if instance.writer != Some(id) {
            instance.waiting.retain(|waiting| *waiting != id);
            self.agents.events.push(WorkflowEvent::AgentRequestSettled {
                instance: turn.reference.id,
                request: id.0,
                committed: false,
                version: instance.version,
            });
            return;
        }
        if completed {
            instance.committed = Some(turn.state.expect("admitted update state"));
            instance.version = instance
                .version
                .checked_add(1)
                .expect("agent version exhausted");
        }
        self.agents.events.push(WorkflowEvent::AgentRequestSettled {
            instance: turn.reference.id,
            request: id.0,
            committed: completed,
            version: instance.version,
        });
        instance.writer = None;
        while let Some(next) = instance.waiting.pop_front() {
            let task = self.tasks.get_mut(&next).expect("queued request");
            if task.state != TaskState::Waiting(TaskWait::Agent(turn.reference.id)) {
                continue;
            }
            instance.writer = Some(next);
            task.local.agent_turn.as_mut().expect("queued update").state = Some(
                instance
                    .committed
                    .as_ref()
                    .expect("published Agent state")
                    .clone(),
            );
            task.state = TaskState::Ready;
            self.agents
                .events
                .push(WorkflowEvent::AgentRequestAdmitted {
                    instance: turn.reference.id,
                    request: next.0,
                    updating: true,
                    version: instance.version,
                });
            self.enqueue(next);
            break;
        }
    }

    pub(crate) fn take_agent_events(&mut self) -> Vec<WorkflowEvent> {
        std::mem::take(&mut self.agents.events)
    }

    pub(super) fn has_agent_instances(&self) -> bool {
        self.agents
            .instances
            .values()
            .any(|instance| !instance.lifetime.is_closed())
    }
}

impl TaskScheduler {
    pub(crate) fn mark_autonomous_main(
        &mut self,
        handle: TaskHandle,
        target: crate::control::CallTarget,
        args: Vec<InterpValue>,
        span: etas_core::Span,
        completion: crate::control::Continuation,
    ) -> Result<(), SchedulerError> {
        let id = self.resolve_id(handle)?;
        let mut machine = EvalMachine::new();
        machine.resume(crate::control::ControlSignal::pending_call(
            crate::control::PendingCall {
                purity: crate::control::CallPurity::Ordinary,
                site: None,
                target,
                args,
                span,
                continuation: completion,
            },
        ));
        self.tasks.get_mut(&id).expect("automatic request").machine = machine;
        assert!(
            self.agents.autonomous.insert(id),
            "automatic main is started once"
        );
        Ok(())
    }
    pub(super) fn is_autonomous_main(&self, task: TaskId) -> bool {
        self.agents.autonomous.contains(&task)
    }
}

impl TaskScheduler {
    /// At quiescence, find an exclusive mailbox request whose writer cannot
    /// progress without that same request. Host/CPU waits contribute no edge.
    pub(crate) fn agent_wait_cycle(&self) -> Option<String> {
        for (&queued, task) in &self.tasks {
            let TaskState::Waiting(TaskWait::Agent(instance)) = task.state else {
                continue;
            };
            let Some(writer) = self
                .agents
                .instances
                .get(&instance)
                .and_then(|agent| agent.writer)
            else {
                continue;
            };
            if let Some(path) = self.required_wait_path(writer, queued) {
                let requests = path.iter().map(|id| id.0.to_string()).collect::<Vec<_>>();
                return Some(format!(
                    "Agent request deadlock. Request {} waits for the update turn on Agent instance {} held by request {}, which waits along task path {}.",
                    queued.0,
                    instance,
                    writer.0,
                    requests.join(" -> "),
                ));
            }
        }
        None
    }

    fn required_wait_path(&self, start: TaskId, finish: TaskId) -> Option<Vec<TaskId>> {
        // Starting with the queued request, propagate which tasks cannot
        // proceed without it. A scope join needs all its children to settle;
        // a completion consumer can proceed after any eligible child settles.
        let mut blocked = BTreeMap::from([(finish, finish)]);
        while !blocked.contains_key(&start) {
            let before = blocked.len();
            for (&id, task) in &self.tasks {
                if blocked.contains_key(&id) {
                    continue;
                }
                let TaskState::Waiting(wait) = task.state else {
                    continue;
                };
                if let Some(dependency) = self.blocking_task(wait, &blocked) {
                    blocked.insert(id, dependency);
                }
            }
            if blocked.len() == before {
                return None;
            }
        }
        let mut path = vec![start];
        let mut task = start;
        while task != finish {
            task = blocked[&task];
            path.push(task);
        }
        Some(path)
    }

    fn blocking_task(&self, wait: TaskWait, blocked: &BTreeMap<TaskId, TaskId>) -> Option<TaskId> {
        match wait {
            TaskWait::Task(target) => blocked.contains_key(&target).then_some(target),
            TaskWait::Agent(instance) => self
                .agents
                .instances
                .get(&instance)?
                .writer
                .filter(|writer| blocked.contains_key(writer)),
            TaskWait::Scope(scope) => self.groups[&scope]
                .tasks
                .iter()
                .find(|child| !self.tasks[child].state.is_terminal() && blocked.contains_key(child))
                .copied(),
            TaskWait::Completion(scope) => {
                let group = &self.groups[&scope];
                if !group.completions.is_empty() {
                    return None;
                }
                let mut children = group.tasks.iter().copied().filter(|child| {
                    !self.tasks[child].state.is_terminal() && self.tasks[child].start.is_some()
                });
                let first = children.next()?;
                (blocked.contains_key(&first) && children.all(|child| blocked.contains_key(&child)))
                    .then_some(first)
            }
            TaskWait::HandlerOwner(owner) => blocked.contains_key(&owner).then_some(owner),
            _ => None,
        }
    }
}

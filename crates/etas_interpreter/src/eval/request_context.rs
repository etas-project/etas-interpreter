use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use etas_host::{ApprovalGrant, AuthorityContext, execution::ExecutionScope};

use super::TaskLocalState;
use crate::{
    api::HostExecutionContext,
    driver::scheduler::{TaskHandle, TaskIdentity, TaskScopeId},
    orchestration::HandlerScopeId,
};

pub(crate) type ApprovalLedger = Rc<RefCell<Vec<ApprovalGrant>>>;

/// Host state belongs to the Agent request even when its Handler runs on the
/// lexical owner's machine. Budget clones retain the original live accounts.
#[derive(Clone, Debug)]
pub(crate) struct AgentHostContext {
    host: HostExecutionContext,
    constraints: Vec<AuthorityContext>,
    model_restrictions: Vec<super::model_restrictions::AgentModelRestrictions>,
    approvals: ApprovalLedger,
    request: TaskIdentity,
    execution: ExecutionScope,
    initialization: Option<Rc<crate::control::AgentInitialization>>,
    initialization_owner: Option<Rc<crate::control::AgentLifetime>>,
    record_constructions: Vec<Weak<crate::control::RecordConstruction>>,
    task: TaskIdentity,
    scope: TaskScopeId,
    operations: Rc<Cell<usize>>,
    resource_owner: Option<(TaskHandle, ExecutionScope)>,
}

#[derive(Debug)]
pub(crate) struct BorrowedAgentHostContext {
    context: AgentHostContext,
    handler: Option<HandlerScopeId>,
    active: Cell<bool>,
    continuation: RefCell<Option<super::machine::WeakTaskHandlerContinuation>>,
}

#[derive(Debug)]
pub(crate) struct AgentHostContextLease(Rc<BorrowedAgentHostContext>);

impl AgentHostContextLease {
    pub(crate) fn attach(&self, continuation: &super::machine::TaskHandlerContinuation) {
        *self.0.continuation.borrow_mut() = Some(continuation.downgrade());
    }
}

impl Drop for AgentHostContextLease {
    fn drop(&mut self) {
        self.0.active.set(false);
    }
}

impl TaskLocalState {
    pub(crate) fn host_execution(&self) -> ExecutionScope {
        if let Some(initialization) = self.pending_agent_initialization() {
            return initialization.lifetime.execution.clone();
        }
        self.borrowed_host_context().map_or_else(
            || self.execution.clone(),
            |borrowed| borrowed.context.execution.clone(),
        )
    }

    pub(crate) fn pending_agent_initialization(
        &self,
    ) -> Option<Rc<crate::control::AgentInitialization>> {
        let task = self.record_task_identity();
        self.agent_initializations
            .iter()
            .rev()
            .filter_map(Weak::upgrade)
            .find(|init| init.task == task && init.is_pending())
            .or_else(|| {
                self.borrowed_host_context()
                    .and_then(|context| context.context.initialization.clone())
                    .filter(|init| init.is_pending())
            })
    }

    pub(crate) fn agent_initialization_owner(&self) -> Option<Rc<crate::control::AgentLifetime>> {
        self.pending_agent_initialization()
            .map(|init| init.lifetime.clone())
            .or_else(|| {
                self.borrowed_host_context()
                    .and_then(|context| context.context.initialization_owner.clone())
            })
            .or_else(|| self.agent_initialization_owner.clone())
    }

    pub(crate) fn register_record_construction(
        &mut self,
        guard: &Rc<crate::control::RecordConstruction>,
    ) {
        self.record_constructions
            .retain(|weak| weak.upgrade().is_some_and(|guard| guard.is_pending()));
        if !self
            .record_constructions
            .iter()
            .filter_map(Weak::upgrade)
            .any(|existing| Rc::ptr_eq(&existing, guard))
        {
            self.record_constructions.push(Rc::downgrade(guard));
        }
    }

    pub(crate) fn pending_record_constructions(
        &self,
    ) -> Vec<Rc<crate::control::RecordConstruction>> {
        let task = self.record_task_identity();
        let mut guards: Vec<_> = self
            .record_constructions
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|guard| guard.task == task && guard.is_pending())
            .collect();
        let borrowed_record = self.borrowed_record_context();
        let borrowed = self.borrowed_host_context();
        let inherited = borrowed_record
            .as_ref()
            .map(|context| &context.guards)
            .unwrap_or_else(|| {
                borrowed
                    .as_ref()
                    .map_or(&self.inherited_record_constructions, |borrowed| {
                        &borrowed.context.record_constructions
                    })
            });
        for guard in inherited
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|guard| guard.is_pending())
        {
            if !guards.iter().any(|existing| Rc::ptr_eq(existing, &guard)) {
                guards.push(guard);
            }
        }
        guards
    }

    fn borrowed_record_context(&self) -> Option<Rc<crate::control::RecordContext>> {
        self.borrowed_record_contexts
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|context| context.is_active())
            .max_by_key(|context| context.order)
    }
    pub(crate) fn record_task_identity(&self) -> TaskIdentity {
        self.borrowed_record_context()
            .map_or_else(|| self.host_task_identity(), |context| context.source)
    }
    pub(crate) fn capture_record_context(&self) -> Option<crate::control::RecordContextInput> {
        let guards = self.pending_record_constructions();
        // An empty source context also masks an unrelated constructor that is
        // suspended on the lexical Handler owner's machine.
        Some((
            self.record_task_identity(),
            guards.iter().map(Rc::downgrade).collect(),
        ))
    }
    pub(crate) fn borrow_record_context(
        &mut self,
        source: TaskIdentity,
        guards: Vec<Weak<crate::control::RecordConstruction>>,
    ) -> crate::control::RecordContextLease {
        let lease = crate::control::RecordContextLease::new(self.identity, source, guards);
        self.borrowed_record_contexts
            .retain(|weak| weak.upgrade().is_some_and(|context| context.is_active()));
        self.borrowed_record_contexts.push(Rc::downgrade(&lease.0));
        lease
    }

    pub(crate) fn host_task_identity(&self) -> TaskIdentity {
        self.borrowed_host_context()
            .map_or(self.identity, |borrowed| borrowed.context.task)
    }

    pub(crate) fn host_task_scope(&self) -> TaskScopeId {
        self.borrowed_host_context()
            .map_or(self.task_scope, |borrowed| borrowed.context.scope)
    }

    pub(crate) fn host_operations(&self) -> Rc<Cell<usize>> {
        self.borrowed_host_context().map_or_else(
            || self.host_operations.clone(),
            |borrowed| borrowed.context.operations.clone(),
        )
    }

    pub(crate) fn host_resource_owner(&self) -> Option<(TaskHandle, ExecutionScope)> {
        self.borrowed_host_context().map_or_else(
            || self.resource_owner.clone(),
            |borrowed| borrowed.context.resource_owner.clone(),
        )
    }

    pub(crate) fn cancelled_agent_handler(
        &self,
    ) -> Result<Option<super::machine::TaskHandlerContinuation>, etas_host::HostError> {
        if self.borrowed_host_contexts.is_empty() {
            return Ok(None);
        }
        if self
            .cancellation
            .as_ref()
            .map_err(Clone::clone)?
            .is_cancelled()
        {
            return Ok(None);
        }
        for borrowed in self
            .borrowed_host_contexts
            .iter()
            .rev()
            .filter_map(Weak::upgrade)
        {
            if borrowed.active.get()
                && borrowed.context.execution.signal()?.is_cancelled()
                && let Some(continuation) = borrowed.continuation.borrow().as_ref()
                && let Some(continuation) = continuation.upgrade()
            {
                return Ok(Some(continuation));
            }
        }
        Ok(None)
    }

    pub(crate) fn host_budget(&self) -> etas_host::ExecutionBudget {
        self.borrowed_host_context().map_or_else(
            || self.host_context.budget.clone(),
            |borrowed| borrowed.context.host.budget.clone(),
        )
    }

    pub(crate) fn host_trace(&self) -> etas_host::TraceContext {
        self.borrowed_host_context().map_or_else(
            || self.host_context.trace.clone(),
            |borrowed| borrowed.context.host.trace.clone(),
        )
    }

    fn borrowed_host_context(&self) -> Option<Rc<BorrowedAgentHostContext>> {
        self.borrowed_host_contexts
            .iter()
            .rev()
            .filter_map(Weak::upgrade)
            .find(|context| context.active.get())
    }

    pub(crate) fn host_context(&self) -> HostExecutionContext {
        let (mut host, approvals) = match self.borrowed_host_context() {
            Some(borrowed) => (
                borrowed.context.host.clone(),
                borrowed.context.approvals.clone(),
            ),
            None => (self.host_context.clone(), self.approvals.clone()),
        };
        host.authority.approvals = approvals.borrow().clone();
        host
    }

    pub(crate) fn authority_constraints(&self) -> Vec<AuthorityContext> {
        self.borrowed_host_context().map_or_else(
            || self.authority_constraints.clone(),
            |borrowed| borrowed.context.constraints.clone(),
        )
    }

    pub(crate) fn model_restrictions(
        &self,
    ) -> Vec<super::model_restrictions::AgentModelRestrictions> {
        self.borrowed_host_context().map_or_else(
            || self.model_restrictions.clone(),
            |borrowed| borrowed.context.model_restrictions.clone(),
        )
    }

    pub(crate) fn approval_ledger(&self) -> ApprovalLedger {
        self.borrowed_host_context().map_or_else(
            || self.approvals.clone(),
            |borrowed| borrowed.context.approvals.clone(),
        )
    }

    pub(crate) fn agent_request_root(&self) -> Option<TaskIdentity> {
        self.borrowed_host_context()
            .map(|borrowed| borrowed.context.request)
            .or(self.agent_request_root)
    }

    pub(crate) fn capture_agent_host_context(&self) -> Option<AgentHostContext> {
        if let Some(borrowed) = self.borrowed_host_context() {
            let mut context = borrowed.context.clone();
            context.initialization = self.pending_agent_initialization();
            context.initialization_owner = self.agent_initialization_owner();
            context.record_constructions = self
                .pending_record_constructions()
                .iter()
                .map(Rc::downgrade)
                .collect();
            return Some(context);
        }
        Some(AgentHostContext {
            host: self.host_context(),
            constraints: self.authority_constraints.clone(),
            model_restrictions: self.model_restrictions.clone(),
            approvals: self.approvals.clone(),
            request: self.agent_request_root.or_else(|| {
                self.agent_initialization_owner()
                    .map(|_| self.host_task_identity())
            })?,
            // The loan's cancellation belongs to the issuing task. Native
            // ownership can select an initialization scope through the fields
            // below without detaching the Handler from that task's stop token.
            execution: self.execution.clone(),
            initialization: self.pending_agent_initialization(),
            initialization_owner: self.agent_initialization_owner(),
            record_constructions: self
                .pending_record_constructions()
                .iter()
                .map(Rc::downgrade)
                .collect(),
            task: self.identity,
            scope: self.task_scope,
            operations: self.host_operations.clone(),
            resource_owner: self.resource_owner.clone(),
        })
    }

    pub(crate) fn borrow_agent_host_context(
        &mut self,
        context: AgentHostContext,
        handler: Option<HandlerScopeId>,
    ) -> AgentHostContextLease {
        self.borrowed_host_contexts
            .retain(|weak| weak.upgrade().is_some_and(|context| context.active.get()));
        let borrowed = Rc::new(BorrowedAgentHostContext {
            context,
            handler,
            active: Cell::new(true),
            continuation: RefCell::new(None),
        });
        self.borrowed_host_contexts.push(Rc::downgrade(&borrowed));
        AgentHostContextLease(borrowed)
    }

    pub(crate) fn close_agent_handler_context(&self, handler: HandlerScopeId) {
        for borrowed in self.borrowed_host_contexts.iter().filter_map(Weak::upgrade) {
            if borrowed.handler == Some(handler) {
                borrowed.active.set(false);
            }
        }
    }
}

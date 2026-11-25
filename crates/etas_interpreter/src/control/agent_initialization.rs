use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use crate::{driver::scheduler::TaskIdentity, value::AgentReference};
use etas_host::execution::{CancellationReason, ExecutionScope};

/// Instance lifetime is independent of references copied into its state.
#[derive(Debug)]
pub(crate) struct AgentLifetime {
    pub(crate) execution: ExecutionScope,
    pub(crate) owner: crate::driver::scheduler::TaskScopeId,
    children: RefCell<Vec<Weak<AgentLifetime>>>,
    closed: Cell<bool>,
}

impl AgentLifetime {
    pub(crate) fn new(
        execution: ExecutionScope,
        owner: crate::driver::scheduler::TaskScopeId,
        parent: Option<&Rc<Self>>,
    ) -> Rc<Self> {
        let lifetime = Rc::new(Self {
            execution,
            owner,
            children: RefCell::new(Vec::new()),
            closed: Cell::new(false),
        });
        if let Some(parent) = parent {
            parent.children.borrow_mut().push(Rc::downgrade(&lifetime));
        }
        lifetime
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.get()
    }

    pub(crate) fn close(&self) -> Result<(), etas_host::HostError> {
        if self.closed.replace(true) {
            return Ok(());
        }
        self.execution
            .cancel_source()
            .stop(CancellationReason::OwnerDropped)?;
        for child in self.children.borrow().iter().filter_map(Weak::upgrade) {
            child.close()?;
        }
        self.execution.finish_body(true)
    }
}

/// Held by the constructor continuation until its state is published.
/// Dropping an abandoned constructor closes only newly owned descendants.
#[derive(Debug)]
pub struct AgentInitialization {
    pub(crate) reference: AgentReference,
    pub(crate) task: TaskIdentity,
    pub(crate) lifetime: Rc<AgentLifetime>,
    published: Cell<bool>,
}

impl AgentInitialization {
    pub(crate) fn new(
        reference: AgentReference,
        task: TaskIdentity,
        lifetime: Rc<AgentLifetime>,
    ) -> Rc<Self> {
        Rc::new(Self {
            reference,
            task,
            lifetime,
            published: Cell::new(false),
        })
    }
    pub(crate) fn is_pending(&self) -> bool {
        !self.published.get() && !self.lifetime.is_closed()
    }
    pub(crate) fn publish(&self) {
        assert!(self.is_pending(), "Agent constructor already settled");
        self.published.set(true);
    }
    pub(crate) fn abort(&self) -> Result<(), etas_host::HostError> {
        self.lifetime.close()
    }
}

impl Drop for AgentInitialization {
    fn drop(&mut self) {
        if !self.published.get() {
            let _ = self.lifetime.close();
        }
    }
}

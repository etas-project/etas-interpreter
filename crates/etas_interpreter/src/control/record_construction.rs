use super::AgentLifetime;
use crate::driver::scheduler::TaskIdentity;
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    sync::atomic::{AtomicU64, Ordering},
};

/// Records borrow values, but own the cleanup of fresh Agents until completed.
#[derive(Debug)]
pub struct RecordConstruction {
    pub(crate) id: u64,
    pub(crate) task: TaskIdentity,
    pub(crate) owner: crate::driver::scheduler::TaskScopeId,
    execution: RefCell<Option<etas_host::execution::ExecutionScope>>,
    pending: Cell<bool>,
    children: RefCell<Vec<Weak<AgentLifetime>>>,
}

impl RecordConstruction {
    pub(crate) fn new(
        task: TaskIdentity,
        owner: crate::driver::scheduler::TaskScopeId,
    ) -> Rc<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Rc::new(Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            task,
            owner,
            execution: RefCell::new(None),
            pending: Cell::new(true),
            children: RefCell::new(Vec::new()),
        })
    }
    pub(crate) fn bind_execution(&self, execution: etas_host::execution::ExecutionScope) {
        *self.execution.borrow_mut() = Some(execution);
    }
    pub(crate) fn execution(&self) -> etas_host::execution::ExecutionScope {
        self.execution
            .borrow()
            .as_ref()
            .expect("record constructor owner bound before evaluation")
            .clone()
    }
    pub(crate) fn is_pending(&self) -> bool {
        self.pending.get()
    }
    pub(crate) fn register(&self, child: &Rc<AgentLifetime>) {
        assert!(self.is_pending());
        self.children.borrow_mut().push(Rc::downgrade(child));
    }
    pub(crate) fn publish(&self) {
        self.pending.set(false);
        self.children.borrow_mut().clear();
    }
    pub(crate) fn abort(&self) -> Result<(), etas_host::HostError> {
        if !self.pending.replace(false) {
            return Ok(());
        }
        for child in self.children.borrow().iter().filter_map(Weak::upgrade) {
            child.close()?;
        }
        Ok(())
    }
    pub(crate) fn checkpoint(&self) -> Result<(), String> {
        if self
            .children
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .any(|child| !child.is_closed())
        {
            return Err("cannot checkpoint record construction owning live Agent instances".into());
        }
        Ok(())
    }
}
pub(crate) type RecordContextInput = (TaskIdentity, Vec<Weak<RecordConstruction>>);

#[derive(Debug)]
pub(crate) struct RecordContext {
    pub(crate) owner: TaskIdentity,
    pub(crate) source: TaskIdentity,
    pub(crate) order: u64,
    pub(crate) guards: Vec<Weak<RecordConstruction>>,
    active: Cell<bool>,
}

static NEXT_CONTEXT: AtomicU64 = AtomicU64::new(1);
#[derive(Debug)]
pub(crate) struct RecordContextLease(pub(crate) Rc<RecordContext>);
impl RecordContextLease {
    pub(crate) fn new(
        owner: TaskIdentity,
        source: TaskIdentity,
        guards: Vec<Weak<RecordConstruction>>,
    ) -> Self {
        Self(Rc::new(RecordContext {
            owner,
            source,
            guards,
            order: NEXT_CONTEXT.fetch_add(1, Ordering::Relaxed),
            active: Cell::new(true),
        }))
    }
    pub(crate) fn restore(
        owner: TaskIdentity,
        source: TaskIdentity,
        order: u64,
        guards: Vec<Weak<RecordConstruction>>,
    ) -> Result<Self, String> {
        let next = order
            .checked_add(1)
            .ok_or("record context identity exhausted")?;
        NEXT_CONTEXT.fetch_max(next, Ordering::Relaxed);
        Ok(Self(Rc::new(RecordContext {
            owner,
            source,
            guards,
            order,
            active: Cell::new(true),
        })))
    }
}
impl RecordContext {
    pub(crate) fn is_active(&self) -> bool {
        self.active.get()
    }
}
impl Drop for RecordContextLease {
    fn drop(&mut self) {
        self.0.active.set(false);
    }
}

impl Drop for RecordConstruction {
    fn drop(&mut self) {
        let _ = self.abort();
    }
}

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use etas_hir::SymbolId;

use super::Frame;
use crate::value::InterpValue;

#[derive(Clone, Debug, Default)]
pub(crate) struct ReceiverAccessSet(Rc<RefCell<Vec<std::rc::Weak<Lease>>>>);

impl ReceiverAccessSet {
    pub(crate) fn track(&self, lease: &ReceiverLease) {
        let mut leases = self.0.borrow_mut();
        leases.retain(|lease| lease.upgrade().is_some_and(|lease| lease.active.get()));
        leases.push(Rc::downgrade(&lease.0));
    }

    pub(crate) fn is_held(&self) -> bool {
        self.0
            .borrow()
            .iter()
            .any(|lease| lease.upgrade().is_some_and(|lease| lease.active.get()))
    }

    pub(crate) fn abort_all(&self) {
        let leases: Vec<_> = self
            .0
            .borrow()
            .iter()
            .filter_map(std::rc::Weak::upgrade)
            .collect();
        for lease in leases {
            ReceiverLease(lease).abort();
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReceiverLease(Rc<Lease>);

#[derive(Debug)]
struct Lease {
    frame: Frame,
    symbol: SymbolId,
    active: Cell<bool>,
}

impl ReceiverLease {
    pub(super) fn new(frame: Frame, symbol: SymbolId) -> Self {
        Self(Rc::new(Lease {
            frame,
            symbol,
            active: Cell::new(true),
        }))
    }

    pub(crate) fn abort(&self) {
        if self.0.active.replace(false) {
            self.0.frame.release_receiver(self.0.symbol);
        }
    }

    pub(crate) fn commit(&self, value: InterpValue) -> Result<(), String> {
        if !self.0.active.replace(false) {
            return Err("receiver update has already completed".into());
        }
        self.0.frame.release_receiver(self.0.symbol);
        let mut frame = self.0.frame.clone();
        if !frame.set(self.0.symbol, value) {
            return Err("updating receiver binding no longer exists".into());
        }
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.active.get() {
            self.frame.release_receiver(self.symbol);
        }
    }
}

/// The callee publishes its final local value; the caller commits only after
/// the complete invocation and any code-owner conversions return normally.
#[derive(Clone, Debug)]
pub struct ReceiverOutcome {
    pub(crate) slot: u32,
    value: Rc<RefCell<Option<InterpValue>>>,
}

impl ReceiverOutcome {
    pub(crate) fn new(slot: u32) -> Self {
        Self {
            slot,
            value: Rc::new(RefCell::new(None)),
        }
    }

    pub(crate) fn store(&self, value: InterpValue) {
        *self.value.borrow_mut() = Some(value);
    }

    pub(crate) fn take(&self) -> Result<InterpValue, String> {
        self.value
            .borrow_mut()
            .take()
            .ok_or_else(|| "updating method did not publish its final receiver".into())
    }
}

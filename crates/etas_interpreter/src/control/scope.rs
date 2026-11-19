use std::{cell::Cell, collections::HashSet, rc::Rc};

use etas_hir::SymbolId;

use super::Frame;

/// Shared by continuation copies, rather than by captured callable values.
/// Abandoning the final continuation ends its lexical binding scope too.
#[derive(Debug)]
pub struct ScopeExitGuard {
    frame: Frame,
    keep: Rc<HashSet<SymbolId>>,
    closed: Cell<bool>,
}

impl ScopeExitGuard {
    pub(crate) fn new(frame: Frame, keep: Rc<HashSet<SymbolId>>) -> Rc<Self> {
        Rc::new(Self {
            frame,
            keep,
            closed: Cell::new(false),
        })
    }

    pub(crate) fn close(&self) {
        self.closed.set(true);
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.get()
    }

    pub(crate) fn matches(&self, frame: &Frame, keep: &HashSet<SymbolId>) -> bool {
        self.frame.capture_identity() == frame.capture_identity() && self.keep.as_ref() == keep
    }
}

impl Drop for ScopeExitGuard {
    fn drop(&mut self) {
        if !self.closed.get() {
            self.frame.clone().cleanup_to(&self.keep);
        }
    }
}

//! Engine adapter between the evaluation machine and the shared residual
//! monitor kernel.
//!
//! The machine owns scope instances in explicit frames. A mirror of those
//! frames, maintained by every machine stack mutation, tells the evaluator
//! which scopes dynamically enclose the code it is running, including the
//! revision boundaries an action must be relocated across. Every governed
//! action is observed at its first dispatch point, before any handler,
//! default implementation or Host boundary runs, and its outcome is
//! correlated through a continuation installed at that point.

mod event;
mod exit;
mod scope;

use std::cell::RefCell;
use std::rc::Rc;

use etas_code::residual::monitor::{ActionAttemptId, ScopeInvocationId, SemanticEventCursor};

use crate::control::residual::{ResidualRunLog, ResidualStateLimits, SharedResidualScope};

/// One machine frame that matters to residual observation.
pub(crate) enum ResidualMirrorEntry {
    Scope(SharedResidualScope),
    /// A revision call boundary. Actions crossing it toward the parent are
    /// relocated with the boundary's checked action projections.
    Boundary(Result<ResidualBoundary, String>),
}

#[derive(Clone)]
pub(crate) struct ResidualBoundary {
    pub(crate) projections: Vec<crate::plan::code::RevisionProjection>,
    pub(crate) parent: crate::value::CheckedExecutionProgram,
}

/// Bottom-to-top mirror of residual scope and revision boundary frames.
#[derive(Default)]
pub(crate) struct ResidualMirror {
    entries: Vec<ResidualMirrorEntry>,
    scopes: usize,
}

impl ResidualMirror {
    pub(crate) fn push(&mut self, entry: ResidualMirrorEntry) {
        if matches!(entry, ResidualMirrorEntry::Scope(_)) {
            self.scopes += 1;
        }
        self.entries.push(entry);
    }

    pub(crate) fn pop(&mut self) -> Option<ResidualMirrorEntry> {
        let entry = self.entries.pop()?;
        if matches!(entry, ResidualMirrorEntry::Scope(_)) {
            self.scopes -= 1;
        }
        Some(entry)
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.scopes = 0;
    }

    pub(crate) fn has_scopes(&self) -> bool {
        self.scopes > 0
    }

    pub(crate) fn scope_count(&self) -> usize {
        self.scopes
    }

    /// Entries from the innermost (top) frame outward.
    pub(crate) fn top_down(&self) -> impl Iterator<Item = &ResidualMirrorEntry> {
        self.entries.iter().rev()
    }
}

/// Run-scoped residual state shared by every program scope of one run.
pub(crate) struct ResidualRunState {
    pub(crate) mirror: Rc<RefCell<ResidualMirror>>,
    pub(crate) log: Rc<ResidualRunLog>,
    pub(crate) limits: ResidualStateLimits,
    next_cursor: u64,
    next_attempt: u64,
    next_invocation: u64,
}

impl Default for ResidualRunState {
    fn default() -> Self {
        let limits = ResidualStateLimits::default();
        Self {
            mirror: Rc::default(),
            log: ResidualRunLog::new(limits.max_exit_records),
            limits,
            next_cursor: 0,
            next_attempt: 0,
            next_invocation: 0,
        }
    }
}

impl std::fmt::Debug for ResidualRunState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResidualRunState")
            .field("scopes", &self.mirror.borrow().scope_count())
            .field("next_cursor", &self.next_cursor)
            .finish_non_exhaustive()
    }
}

impl ResidualRunState {
    pub(crate) fn next_cursor(&mut self) -> Result<SemanticEventCursor, String> {
        self.next_cursor = self
            .next_cursor
            .checked_add(1)
            .ok_or("residual event cursor is exhausted")?;
        Ok(SemanticEventCursor(self.next_cursor))
    }

    pub(crate) fn next_attempt(&mut self) -> Result<ActionAttemptId, String> {
        self.next_attempt = self
            .next_attempt
            .checked_add(1)
            .ok_or("residual attempt identities are exhausted")?;
        Ok(ActionAttemptId::from_bytes(identity_bytes(
            b'a',
            self.next_attempt,
        )))
    }

    pub(crate) fn next_invocation(&mut self) -> Result<ScopeInvocationId, String> {
        self.next_invocation = self
            .next_invocation
            .checked_add(1)
            .ok_or("residual scope invocation identities are exhausted")?;
        Ok(ScopeInvocationId::from_bytes(identity_bytes(
            b's',
            self.next_invocation,
        )))
    }

    /// `(cursor, attempt, invocation)` counters for an engine checkpoint.
    pub(crate) fn counters(&self) -> (u64, u64, u64) {
        (self.next_cursor, self.next_attempt, self.next_invocation)
    }

    /// Continues counting after a restored checkpoint; counters only grow.
    pub(crate) fn restore_counters(&mut self, counters: (u64, u64, u64)) {
        self.next_cursor = self.next_cursor.max(counters.0);
        self.next_attempt = self.next_attempt.max(counters.1);
        self.next_invocation = self.next_invocation.max(counters.2);
    }
}

fn identity_bytes(kind: u8, value: u64) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[0] = kind;
    bytes[8..].copy_from_slice(&value.to_be_bytes());
    bytes
}

pub(in crate::eval) fn invocation_counter(id: &[u8; 16]) -> Result<std::num::NonZeroU64, String> {
    if id[0] != b's' || id[1..8] != [0; 7] {
        return Err("residual scope invocation is not an engine-owned identity".into());
    }
    let mut counter = [0; 8];
    counter.copy_from_slice(&id[8..]);
    std::num::NonZeroU64::new(u64::from_be_bytes(counter))
        .ok_or_else(|| "residual scope invocation must be nonzero".into())
}

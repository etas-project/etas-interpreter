use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use etas_code::{
    RevisionKey,
    residual::monitor::{ScopeCheckResult, ScopeExit, ScopeInvocationId},
};
use etas_core::{AnalysisDiagnosticCode, Diagnostic, Span};
use etas_hir::HirItemId;

use super::{SharedResidualScope, state::ResidualScopeInstance};
use crate::control::ExecutionFault;
use crate::orchestration::{
    ResidualExitLogSnapshot, ResidualExitRecordSnapshot, ResidualExitSnapshot,
};
use crate::plan::residual::PreparedResidualPlans;

/// Suspension and checkpointing never exit a scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResidualExitKind {
    Return,
    NonLocalReturn,
    ErrorPropagation,
    HandlerFinish,
    Abort,
    Cancellation,
    Fault,
}

impl ResidualExitKind {
    pub(super) fn monitor_exit(self) -> ScopeExit {
        match self {
            Self::Return => ScopeExit::Completed,
            Self::Cancellation => ScopeExit::Cancelled,
            _ => ScopeExit::Failed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResidualExitRecord {
    pub(crate) item: HirItemId,
    pub(crate) revision: RevisionKey,
    pub(crate) obligations: Vec<etas_code::residual::identity::ObligationId>,
    pub(crate) span: Span,
    pub(crate) invocation: ScopeInvocationId,
    pub(crate) kind: ResidualExitKind,
    pub(crate) result: ScopeCheckResult,
    pub(crate) unknown_outcomes: usize,
}

#[derive(Clone, Debug)]
struct ScopeEntry {
    scope: Weak<RefCell<ResidualScopeInstance>>,
    span: Span,
}

/// A bounded run-local index, not an additional scope owner. Frames and
/// captured continuations own the states; weak entries cover both at terminal
/// unwind, including scopes absent from the active machine's frame mirror.
#[derive(Debug)]
pub(crate) struct ResidualRunLog {
    scopes: RefCell<Vec<ScopeEntry>>,
    exits: RefCell<VecDeque<ResidualExitRecord>>,
    first_unsatisfied: RefCell<Option<ResidualExitRecord>>,
    unsatisfied: Cell<u64>,
    dropped: Cell<u64>,
    capacity: usize,
}

impl ResidualRunLog {
    pub(crate) fn new(capacity: usize) -> Rc<Self> {
        Rc::new(Self {
            scopes: RefCell::default(),
            exits: RefCell::default(),
            first_unsatisfied: RefCell::default(),
            unsatisfied: Cell::new(0),
            dropped: Cell::new(0),
            capacity,
        })
    }

    pub(crate) fn check_activation_capacity(&self, maximum: usize) -> Result<(), String> {
        let mut entries = self
            .scopes
            .try_borrow_mut()
            .map_err(|_| "residual live scope index is busy")?;
        let mut active = Vec::with_capacity(entries.len());
        for entry in entries.iter() {
            if let Some(scope) = entry.scope.upgrade()
                && scope
                    .try_borrow()
                    .map_err(|_| "residual scope is busy at activation")?
                    .is_active()
            {
                active.push(entry.clone());
            }
        }
        *entries = active;
        if entries.len() >= maximum {
            return Err("residual active scope ceiling exceeded".into());
        }
        Ok(())
    }

    pub(crate) fn register(
        &self,
        scope: &SharedResidualScope,
        maximum: usize,
    ) -> Result<(), String> {
        self.check_activation_capacity(maximum)?;
        let span = scope
            .try_borrow()
            .map_err(|_| "residual scope is busy at registration")?
            .span();
        let mut entries = self
            .scopes
            .try_borrow_mut()
            .map_err(|_| "residual live scope index is busy")?;
        let weak = Rc::downgrade(scope);
        if entries.iter().any(|entry| entry.scope.ptr_eq(&weak)) {
            return Err("residual scope was registered twice".into());
        }
        entries.push(ScopeEntry { scope: weak, span });
        Ok(())
    }

    /// Used only at terminal execution, never at suspension. Continue after
    /// an invariant failure to settle independent scopes as well.
    pub(crate) fn finish_active(&self, kind: ResidualExitKind) -> Vec<ExecutionFault> {
        let entries = self.scopes.borrow().clone();
        let mut faults = Vec::new();
        for entry in entries.iter().rev() {
            let Some(scope) = entry.scope.upgrade() else {
                continue;
            };
            let result = scope
                .try_borrow_mut()
                .map_err(|_| "residual scope is busy at terminal unwind".to_owned())
                .and_then(|mut scope| {
                    if scope.is_active() {
                        scope.finish(kind).map(|_| ())
                    } else {
                        Ok(())
                    }
                });
            if let Err(reason) = result {
                faults.push(ExecutionFault::new(
                    AnalysisDiagnosticCode::TraceSpecResidualIncomplete,
                    entry.span,
                    reason,
                ));
            }
        }
        faults
    }

    pub(super) fn record(&self, record: ResidualExitRecord) {
        if record.result != ScopeCheckResult::Accepting {
            self.unsatisfied
                .set(self.unsatisfied.get().saturating_add(1));
            let mut first = self.first_unsatisfied.borrow_mut();
            if first.is_none() {
                *first = Some(record.clone());
            }
        }
        let mut exits = self.exits.borrow_mut();
        if self.capacity == 0 {
            self.dropped.set(self.dropped.get().saturating_add(1));
            return;
        }
        if exits.len() == self.capacity {
            exits.pop_front();
            self.dropped.set(self.dropped.get().saturating_add(1));
        }
        exits.push_back(record);
    }

    /// Retain a representative failure independently of the bounded audit
    /// ring. Later normal exits cannot erase an earlier unsatisfied scope.
    pub(crate) fn take_diagnostics(&self) -> Vec<Diagnostic> {
        let mut records: Vec<_> = self
            .exits
            .borrow_mut()
            .drain(..)
            .filter(|record| record.result != ScopeCheckResult::Accepting)
            .collect();
        if let Some(first) = self.first_unsatisfied.borrow_mut().take()
            && !records
                .iter()
                .any(|record| record.invocation == first.invocation)
        {
            records.insert(0, first);
        }
        let count = self.unsatisfied.replace(0);
        let dropped = self.dropped.replace(0);
        records.into_iter().map(|record| {
            let message = format!(
                "trace spec residual scope is incomplete after {:?}: {:?}; unknown outcomes: {}",
                record.kind, record.result, record.unknown_outcomes,
            );
            let code = if record.result == ScopeCheckResult::Rejected {
                AnalysisDiagnosticCode::TraceSpecResidualViolation
            } else {
                AnalysisDiagnosticCode::TraceSpecResidualIncomplete
            };
            let mut diagnostic = Diagnostic::analysis(code, record.span, message);
            diagnostic.notes.push(format!(
                "scope invocation {:?}, checked revision {:?}", record.invocation, record.revision,
            ));
            diagnostic.notes.push(format!("affected obligations: {:?}", record.obligations));
            diagnostic.notes.push(format!(
                "unsatisfied scopes: {count}; older audit records omitted: {dropped}",
            ));
            diagnostic
        }).collect()
    }

    pub(crate) fn snapshot(
        &self,
        prepared: &PreparedResidualPlans,
    ) -> Result<ResidualExitLogSnapshot, String> {
        let mut records: Vec<_> = self
            .exits
            .borrow()
            .iter()
            .filter(|record| record.result != ScopeCheckResult::Accepting)
            .cloned()
            .collect();
        if let Some(first) = self.first_unsatisfied.borrow().as_ref()
            && !records
                .iter()
                .any(|record| record.invocation == first.invocation)
        {
            if records.len() == self.capacity.max(1) {
                records.pop();
            }
            records.insert(0, first.clone());
        }
        let records = records
            .into_iter()
            .map(|record| {
                if prepared
                    .scope(record.item)
                    .is_none_or(|scope| scope.revision != record.revision)
                {
                    return Err(
                        "residual exit history is not bound to the checkpoint root's checked plans"
                            .into(),
                    );
                }
                Ok(ResidualExitRecordSnapshot {
                    item: record.item.0,
                    revision: record.revision,
                    invocation: *record.invocation.as_bytes(),
                    exit: ResidualExitSnapshot {
                        kind: super::state::exit_kind_snapshot(record.kind),
                        result: super::state::scope_result_snapshot(record.result),
                    },
                    unknown_outcomes: record.unknown_outcomes,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(ResidualExitLogSnapshot {
            unsatisfied: self.unsatisfied.get(),
            omitted_exits: self.dropped.get(),
            records,
        })
    }

    pub(crate) fn restore_history(
        &self,
        saved: &ResidualExitLogSnapshot,
        prepared: &PreparedResidualPlans,
        checked: &etas_frontend::CheckedProject,
    ) -> Result<(), String> {
        if self.unsatisfied.get() != 0 || !self.exits.borrow().is_empty() {
            return Err("residual exit history was restored into a nonempty run".into());
        }
        if saved.records.len() > self.capacity.max(1) {
            return Err(
                "checkpoint residual exit history exceeds the current record ceiling".into(),
            );
        }
        let records = saved
            .records
            .iter()
            .map(|record| {
                let item = HirItemId(record.item);
                let scope = prepared
                    .scope(item)
                    .filter(|scope| scope.revision == record.revision);
                let Some(scope) = scope else {
                    return Err("checkpoint residual exit history names an unbound revision".into());
                };
                let span = checked
                    .hir
                    .items
                    .get(item)
                    .ok_or("checkpoint residual exit has no checked declaration")?
                    .span();
                Ok(ResidualExitRecord {
                    item,
                    revision: record.revision,
                    obligations: scope.obligation_ids(),
                    span,
                    invocation: ScopeInvocationId::from_bytes(record.invocation),
                    kind: super::state::exit_kind_restore(record.exit.kind),
                    result: super::state::scope_result_restore(record.exit.result),
                    unknown_outcomes: record.unknown_outcomes,
                })
            })
            .collect::<Result<VecDeque<_>, String>>()?;
        *self.first_unsatisfied.borrow_mut() = records.front().cloned();
        *self.exits.borrow_mut() = records;
        self.unsatisfied.set(saved.unsatisfied);
        self.dropped.set(saved.omitted_exits);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn exits(&self) -> Vec<ResidualExitRecord> {
        self.exits.borrow().iter().cloned().collect()
    }
}

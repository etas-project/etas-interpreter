use std::cell::Cell;
use std::rc::Rc;

use etas_code::residual::monitor::{ActionAttemptId, SemanticEventCursor};

use super::{ResidualOutcomeStatus, SharedResidualScope};
use crate::value::InterpValue;

/// One scope that observed and committed an attempt, with the state version
/// produced by that commit.
pub(crate) struct ResidualObservedScope {
    pub(crate) instance: SharedResidualScope,
    pub(crate) action: etas_code::contract::CanonicalEffect,
    pub(crate) version: u64,
}

/// The committed pre-dispatch decision for one semantic action attempt. It
/// is the attempt's only dispatch permit and the correlation anchor of its
/// outcome. Only the engine's observation step constructs it.
pub(crate) struct ResidualAttempt {
    id: ActionAttemptId,
    cursor: SemanticEventCursor,
    scopes: Vec<ResidualObservedScope>,
    decision: bool,
    trusted_approval: Cell<Option<bool>>,
    dispatched: Cell<bool>,
    completed: Cell<bool>,
}

/// Shared handle carried by the pending action and its outcome continuation.
#[derive(Clone)]
pub(crate) struct ResidualAttemptTag(Rc<ResidualAttempt>);

impl std::fmt::Debug for ResidualAttemptTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResidualAttemptTag")
            .field("cursor", &self.0.cursor)
            .field("scopes", &self.0.scopes.len())
            .field("dispatched", &self.0.dispatched.get())
            .field("completed", &self.0.completed.get())
            .finish()
    }
}

impl ResidualAttemptTag {
    pub(crate) fn new(
        id: ActionAttemptId,
        cursor: SemanticEventCursor,
        scopes: Vec<ResidualObservedScope>,
        decision: bool,
    ) -> Self {
        Self(Rc::new(ResidualAttempt {
            id,
            cursor,
            scopes,
            decision,
            trusted_approval: Cell::new(None),
            dispatched: Cell::new(false),
            completed: Cell::new(false),
        }))
    }

    /// Process-local identity for deduplication within one capture.
    pub(crate) fn identity(&self) -> *const () {
        Rc::as_ptr(&self.0).cast::<()>()
    }

    pub(crate) fn observing_scopes(&self) -> impl Iterator<Item = &SharedResidualScope> {
        self.0.scopes.iter().map(|scope| &scope.instance)
    }

    /// Revalidates the permit immediately before the governed dispatch and
    /// consumes it. A permit is single-use and becomes stale if any scope
    /// that committed it exited or took another transition since.
    pub(crate) fn consume_permit(&self) -> Result<(), String> {
        if self.0.dispatched.replace(true) {
            return Err("residual dispatch permit was already used".into());
        }
        for scope in &self.0.scopes {
            let instance = scope
                .instance
                .try_borrow()
                .map_err(|_| "residual scope is busy during dispatch".to_owned())?;
            if !instance.is_active() {
                return Err("residual dispatch permit belongs to an exited scope".into());
            }
            if instance.version() != scope.version {
                return Err("residual dispatch permit is stale".into());
            }
        }
        Ok(())
    }

    /// Exports this attempt for an engine checkpoint; `scope_id` maps each
    /// observing scope to its index in the same checkpoint.
    pub(crate) fn snapshot(
        &self,
        mut scope_id: impl FnMut(&SharedResidualScope) -> Result<u32, String>,
    ) -> Result<crate::orchestration::ResidualAttemptSnapshot, String> {
        Ok(crate::orchestration::ResidualAttemptSnapshot {
            id: *self.0.id.as_bytes(),
            cursor: self.0.cursor.0,
            scopes: self
                .0
                .scopes
                .iter()
                .map(|scope| {
                    Ok(crate::orchestration::ResidualObservedScopeSnapshot {
                        scope: scope_id(&scope.instance)?,
                        action: scope.action.clone(),
                        version: scope.version,
                    })
                })
                .collect::<Result<_, String>>()?,
            dispatched: self.0.dispatched.get(),
            completed: self.0.completed.get(),
        })
    }

    /// Restores a checkpointed attempt against already restored scopes. An
    /// attempt that has not completed must still be pending in every scope
    /// that observed it; a completed one must not be.
    pub(crate) fn restore(
        snapshot: &crate::orchestration::ResidualAttemptSnapshot,
        scopes: &[SharedResidualScope],
    ) -> Result<Self, String> {
        if !snapshot.dispatched {
            return Err(
                "checkpoint has an undispatched residual permit with no safe authorization point"
                    .into(),
            );
        }
        let id = ActionAttemptId::from_bytes(snapshot.id);
        let mut observed = Vec::with_capacity(snapshot.scopes.len());
        let mut decision = None;
        for scope in &snapshot.scopes {
            let instance = scopes
                .get(scope.scope as usize)
                .ok_or("residual checkpoint attempt names an unknown scope")?
                .clone();
            if observed
                .iter()
                .any(|seen: &ResidualObservedScope| Rc::ptr_eq(&seen.instance, &instance))
            {
                return Err("residual checkpoint attempt repeats an observing scope".into());
            }
            let (pending, version) = {
                let borrowed = instance
                    .try_borrow()
                    .map_err(|_| "residual checkpoint scope is busy".to_owned())?;
                borrowed.validate_saved_action(&scope.action)?;
                let current = borrowed.is_approval(&scope.action);
                if decision.is_some_and(|decision| decision != current) {
                    return Err(
                        "checkpoint action projections disagree on approval identity".into(),
                    );
                }
                decision = Some(current);
                let pending = borrowed.pending_action(id);
                if pending.is_some_and(|action| action != &scope.action) {
                    return Err(
                        "residual checkpoint attempt disagrees with its pending action identity"
                            .into(),
                    );
                }
                (pending.is_some(), borrowed.version())
            };
            if pending == snapshot.completed {
                return Err("residual checkpoint attempt disagrees with its scope outcome".into());
            }
            if scope.version > version {
                return Err("residual checkpoint attempt postdates its scope state".into());
            }
            observed.push(ResidualObservedScope {
                instance,
                action: scope.action.clone(),
                version: scope.version,
            });
        }
        let tag = Self::new(
            id,
            SemanticEventCursor(snapshot.cursor),
            observed,
            decision.ok_or("checkpoint residual attempt has no observing scope")?,
        );
        // A resumed action may finish its old continuation, but the old run's
        // approval cannot authorize new work in this invocation.
        tag.0.dispatched.set(snapshot.dispatched);
        tag.0.completed.set(snapshot.completed);
        Ok(tag)
    }

    /// Records the decision of the trusted Host approval boundary for this
    /// attempt. Values resumed by source handlers never reach this method.
    pub(crate) fn record_trusted_approval(&self, approved: bool) {
        self.0.trusted_approval.set(Some(approved));
    }

    /// Correlates the completed outcome with every scope that committed the
    /// attempt. Decision actions are accepted only with a trusted approval
    /// that matches the delivered value; ordinary actions are accepted by
    /// completing with a value. A repeated completion is not a new outcome.
    pub(crate) fn complete(
        &self,
        value: &InterpValue,
        completed_at: SemanticEventCursor,
    ) -> Result<(), String> {
        if self.0.completed.replace(true) {
            return Ok(());
        }
        let status = if self.0.decision {
            match (self.0.trusted_approval.get(), value) {
                (Some(true), InterpValue::Bool(true)) => ResidualOutcomeStatus::Accepted,
                (Some(false), InterpValue::Bool(false)) => ResidualOutcomeStatus::Denied,
                _ => ResidualOutcomeStatus::Failed,
            }
        } else {
            ResidualOutcomeStatus::Accepted
        };
        for scope in &self.0.scopes {
            scope
                .instance
                .try_borrow_mut()
                .map_err(|_| "residual scope is busy while recording an outcome".to_owned())?
                .record_outcome(self.0.id, status, completed_at)?;
        }
        Ok(())
    }
}

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use etas_code::{
    contract::CanonicalEffect,
    residual::{
        binding::BorrowedBindingLimits,
        monitor::{
            AcceptedPriorEvidence, ActionAttemptId, AlternativeScope, ConcreteActionAttempt,
            ConcretePriorEvidence, EvidenceQuery, EvidenceStatus, ScopeCheckResult, ScopeExit,
            ScopeInvocationId, SemanticEventCursor,
        },
        plan::{
            ResidualComponentAttempt, ResidualScopeDecision, ResidualScopeDelta,
            ResidualScopeEvent, ResidualScopeState, SemanticPlanComponent,
        },
    },
};
use etas_frontend::CheckedProgram;

use super::{ResidualExitKind, ResidualExitRecord, ResidualRunLog};
use crate::plan::residual::PreparedResidualScope;

/// Ceilings on live monitor state. Exceeding one stops execution rather than
/// discarding state or choosing a convenient transition.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ResidualStateLimits {
    pub(crate) max_components: usize,
    pub(crate) max_labels: usize,
    pub(crate) max_outcomes: usize,
    pub(crate) max_pending: usize,
    pub(crate) max_active_scopes: usize,
    pub(crate) max_exit_records: usize,
    pub(crate) bindings: BorrowedBindingLimits,
}

impl Default for ResidualStateLimits {
    fn default() -> Self {
        Self {
            max_components: 256,
            max_labels: 4096,
            max_outcomes: 4096,
            max_pending: 256,
            max_active_scopes: 1024,
            max_exit_records: 1024,
            bindings: BorrowedBindingLimits {
                max_bindings: 64,
                max_value_bytes: 64 * 1024,
                max_total_value_bytes: 64 * 1024,
            },
        }
    }
}

/// Activation request for one invocation's scope; carries no state.
#[derive(Clone, Debug)]
pub(crate) struct ResidualScopeActivation {
    pub(crate) scope: Arc<PreparedResidualScope>,
    pub(crate) application: Arc<crate::plan::residual::PreparedResidualApplication>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResidualOutcomeStatus {
    Accepted,
    Denied,
    Failed,
}

struct OutcomeEntry {
    action: CanonicalEffect,
    accepted: Option<AcceptedOutcome>,
    rejected: Option<ResidualOutcomeStatus>,
}

#[derive(Clone, Copy)]
enum AcceptedOutcome {
    CurrentRun(ActionAttemptId, SemanticEventCursor),
    Historical(ActionAttemptId, SemanticEventCursor),
}

impl AcceptedOutcome {
    fn history(self) -> (ActionAttemptId, SemanticEventCursor) {
        match self {
            Self::CurrentRun(id, cursor) | Self::Historical(id, cursor) => (id, cursor),
        }
    }

    fn current(self) -> Option<(ActionAttemptId, SemanticEventCursor)> {
        match self {
            Self::CurrentRun(id, cursor) => Some((id, cursor)),
            Self::Historical(..) => None,
        }
    }
}

struct PendingEntry {
    attempt: ActionAttemptId,
    action: CanonicalEffect,
}

pub(crate) enum ResidualPreview {
    Permit(Vec<ResidualScopeDelta>),
    NeedsEvidence,
    Reject,
}

/// Live state of one scope activation. It shares only immutable plans with
/// other invocations; its registers, evidence and cursor are its own.
pub(crate) struct ResidualScopeInstance {
    prepared: Arc<PreparedResidualScope>,
    application: Arc<crate::plan::residual::PreparedResidualApplication>,
    program: CheckedProgram,
    invocation: ScopeInvocationId,
    span: etas_core::Span,
    states: Vec<ResidualScopeState>,
    labels: HashMap<etas_effects::Effect, CanonicalEffect>,
    outcomes: Vec<OutcomeEntry>,
    pending: Vec<PendingEntry>,
    version: u64,
    last_cursor: Option<SemanticEventCursor>,
    exit: Option<(ResidualExitKind, ScopeCheckResult)>,
    log: Weak<ResidualRunLog>,
    limits: ResidualStateLimits,
}

pub(crate) type SharedResidualScope = Rc<RefCell<ResidualScopeInstance>>;

impl std::fmt::Debug for ResidualScopeInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResidualScopeInstance")
            .field("item", &self.prepared.item)
            .field("invocation", &self.invocation)
            .field("version", &self.version)
            .field("exit", &self.exit)
            .finish_non_exhaustive()
    }
}

impl ResidualScopeInstance {
    pub(crate) fn activate(
        prepared: Arc<PreparedResidualScope>,
        application: Arc<crate::plan::residual::PreparedResidualApplication>,
        program: CheckedProgram,
        invocation: ScopeInvocationId,
        log: &Rc<ResidualRunLog>,
        limits: ResidualStateLimits,
    ) -> Result<Self, String> {
        let span = program
            .hir
            .items
            .get(prepared.item)
            .ok_or("residual scope has no checked declaration")?
            .span();
        validate_component_budget(&prepared, limits.max_components)?;
        let mut states = Vec::with_capacity(prepared.plans.len());
        for (index, plan) in prepared.plans.iter().enumerate() {
            let scope = AlternativeScope {
                scope: plan.scope,
                activation: plan.activation,
                revision: prepared.revision,
                invocation,
            };
            states.push(
                plan.checked
                    .plan()
                    .initial_specialized_scope(
                        scope,
                        limits.max_components,
                        &application.scopes[index],
                    )
                    .map_err(|error| format!("residual scope cannot be activated: {error:?}"))?,
            );
        }
        Ok(Self {
            prepared,
            application,
            program,
            invocation,
            span,
            states,
            labels: HashMap::new(),
            outcomes: Vec::new(),
            pending: Vec::new(),
            version: 0,
            last_cursor: None,
            exit: None,
            log: Rc::downgrade(log),
            limits,
        })
    }

    pub(crate) fn program(&self) -> &CheckedProgram {
        &self.program
    }
    pub(crate) fn span(&self) -> etas_core::Span {
        self.span
    }
    pub(crate) fn version(&self) -> u64 {
        self.version
    }
    pub(crate) fn is_active(&self) -> bool {
        self.exit.is_none()
    }
    pub(crate) fn invocation(&self) -> ScopeInvocationId {
        self.invocation
    }
    pub(crate) fn pending_action(&self, attempt: ActionAttemptId) -> Option<&CanonicalEffect> {
        self.pending
            .iter()
            .find(|entry| entry.attempt == attempt)
            .map(|entry| &entry.action)
    }

    pub(crate) fn is_approval(&self, action: &CanonicalEffect) -> bool {
        self.prepared.is_approval(action)
    }

    pub(crate) fn validate_saved_action(&self, action: &CanonicalEffect) -> Result<(), String> {
        etas_code::residual::pattern::validate_action_instance(
            action,
            etas_code::residual::pattern::ActionPatternLimits {
                max_selectors: self.limits.bindings.max_bindings,
                max_identity_segments: 256,
                max_identity_bytes: self.limits.bindings.max_value_bytes,
                max_literal_bytes: self.limits.bindings.max_total_value_bytes,
            },
        )
        .map_err(|error| format!("checkpoint residual action identity is invalid: {error:?}"))?;
        if !self.application.checked.actions.contains(action) {
            return Err(
                "checkpoint residual action is absent from the checked scope footprint".into(),
            );
        }
        Ok(())
    }

    /// The canonical monitor label of a concrete action checked in this
    /// scope's own program, using the identities that projected its plans.
    pub(crate) fn label(
        &mut self,
        effect: &etas_effects::Effect,
    ) -> Result<CanonicalEffect, String> {
        if let Some(label) = self.labels.get(effect) {
            return Ok(label.clone());
        }
        if self.labels.len() >= self.limits.max_labels {
            return Err("residual scope exceeded its action label ceiling".into());
        }
        let label = self
            .program
            .residual_plans()
            .canonical_action(&self.program, self.prepared.item, effect)
            .map_err(|error| format!("governed action has no residual monitor label: {error:?}"))?;
        self.labels.insert(effect.clone(), label.clone());
        Ok(label)
    }

    /// Evaluates one pre-dispatch attempt against every plan of the scope
    /// without mutating state.
    pub(crate) fn preview(
        &self,
        id: ActionAttemptId,
        cursor: SemanticEventCursor,
        label: &CanonicalEffect,
    ) -> Result<ResidualPreview, String> {
        if self.exit.is_some() {
            return Err("residual scope received an event after it exited".into());
        }
        self.validate_next_cursor(cursor)?;
        let evidence = ScopeEvidence { instance: self };
        let values = BTreeMap::new();
        let mut deltas = Vec::with_capacity(self.states.len());
        for (plan, state) in self.prepared.plans.iter().zip(&self.states) {
            let semantic = plan.checked.plan();
            let mut attempts = BTreeMap::new();
            for component_id in state.component_ids() {
                let Some(SemanticPlanComponent::Trace(component)) =
                    semantic.component(component_id)
                else {
                    return Err("residual scope component is not an installed trace".into());
                };
                let layout = semantic
                    .layout(component.obligation().bindings())
                    .ok_or("residual scope component has no binding layout")?;
                let bindings = layout
                    .validate_borrowed(&values, self.limits.bindings)
                    .map_err(|error| format!("residual action bindings are invalid: {error:?}"))?;
                attempts.insert(
                    component_id,
                    ResidualComponentAttempt::Trace(ConcreteActionAttempt {
                        scope: plan.scope,
                        activation: plan.activation,
                        revision: self.prepared.revision,
                        invocation: self.invocation,
                        id,
                        cursor,
                        action: label,
                        bindings,
                    }),
                );
            }
            match semantic
                .preview_scope(
                    state,
                    ResidualScopeEvent {
                        id,
                        cursor,
                        action: label,
                    },
                    &attempts,
                    &evidence,
                )
                .map_err(|error| format!("residual monitor step failed: {error:?}"))?
            {
                ResidualScopeDecision::Permit(delta) => deltas.push(delta),
                ResidualScopeDecision::NeedsEvidence(_) => {
                    return Ok(ResidualPreview::NeedsEvidence);
                }
                ResidualScopeDecision::Reject => return Ok(ResidualPreview::Reject),
            }
        }
        Ok(ResidualPreview::Permit(deltas))
    }

    /// Commits previously previewed transitions and records the attempt as
    /// dispatched with an outstanding outcome. Returns the new state version.
    pub(crate) fn commit(
        &mut self,
        deltas: Vec<ResidualScopeDelta>,
        attempt: ActionAttemptId,
        cursor: SemanticEventCursor,
        label: CanonicalEffect,
    ) -> Result<u64, String> {
        if self.exit.is_some() {
            return Err("residual scope committed a transition after it exited".into());
        }
        self.validate_next_cursor(cursor)?;
        let version = self
            .version
            .checked_add(1)
            .ok_or("residual scope version is exhausted")?;
        if deltas.len() != self.states.len() {
            return Err("residual commit does not cover every scope plan".into());
        }
        if self.pending.len() >= self.limits.max_pending {
            return Err("residual scope exceeded its pending outcome ceiling".into());
        }
        for ((plan, state), delta) in self
            .prepared
            .plans
            .iter()
            .zip(self.states.iter_mut())
            .zip(deltas)
        {
            plan.checked
                .plan()
                .commit_scope(state, delta)
                .map_err(|error| format!("residual transition is stale: {error:?}"))?;
        }
        self.pending.push(PendingEntry {
            attempt,
            action: label,
        });
        self.version = version;
        self.last_cursor = Some(cursor);
        Ok(self.version)
    }

    /// Records the correlated outcome of an attempt committed in this scope.
    /// A late outcome for an exited scope is ignored; it cannot reopen it.
    pub(crate) fn record_outcome(
        &mut self,
        attempt: ActionAttemptId,
        status: ResidualOutcomeStatus,
        completed_at: SemanticEventCursor,
    ) -> Result<(), String> {
        if self.exit.is_some() {
            return Ok(());
        }
        self.validate_next_cursor(completed_at)?;
        let version = self
            .version
            .checked_add(1)
            .ok_or("residual scope version is exhausted")?;
        let Some(position) = self
            .pending
            .iter()
            .position(|entry| entry.attempt == attempt)
        else {
            return Err("residual outcome does not correlate with a dispatched attempt".into());
        };
        let action = &self.pending[position].action;
        let outcome_index = self
            .outcomes
            .iter()
            .position(|outcome| outcome.action == *action);
        if outcome_index.is_none() && self.outcomes.len() >= self.limits.max_outcomes {
            return Err("residual scope exceeded its evidence ceiling".into());
        }
        let entry = self.pending.remove(position);
        let index = match outcome_index {
            Some(index) => index,
            None => {
                let index = self.outcomes.len();
                self.outcomes.push(OutcomeEntry {
                    action: entry.action,
                    accepted: None,
                    rejected: None,
                });
                index
            }
        };
        let outcome = &mut self.outcomes[index];
        match status {
            ResidualOutcomeStatus::Accepted => {
                outcome.accepted = Some(AcceptedOutcome::CurrentRun(attempt, completed_at))
            }
            status => outcome.rejected = Some(status),
        }
        self.version = version;
        self.last_cursor = Some(completed_at);
        Ok(())
    }

    /// Validates scope acceptance at an exit and records it. A completed
    /// exit returns the monitor result; abnormal exits are recorded without
    /// claiming success.
    pub(crate) fn finish(&mut self, kind: ResidualExitKind) -> Result<ScopeCheckResult, String> {
        if let Some((recorded, result)) = self.exit {
            if recorded == kind || kind == ResidualExitKind::Abort {
                return Ok(result);
            }
            return Err("residual scope exited twice".into());
        }
        let exit = kind.monitor_exit();
        let mut combined = ScopeCheckResult::Accepting;
        for (plan, state) in self.prepared.plans.iter().zip(&self.states) {
            let result = plan
                .checked
                .plan()
                .finish_scope(state, &[], exit, self.limits.max_pending)
                .map_err(|error| format!("residual scope exit check failed: {error:?}"))?;
            combined = match (combined, result) {
                (ScopeCheckResult::Rejected, _) | (_, ScopeCheckResult::Rejected) => {
                    ScopeCheckResult::Rejected
                }
                (ScopeCheckResult::Incomplete, _) | (_, ScopeCheckResult::Incomplete) => {
                    ScopeCheckResult::Incomplete
                }
                (ScopeCheckResult::PendingEvidence, _) | (_, ScopeCheckResult::PendingEvidence) => {
                    ScopeCheckResult::PendingEvidence
                }
                _ => ScopeCheckResult::Accepting,
            };
        }
        if exit != ScopeExit::Completed && combined == ScopeCheckResult::Accepting {
            combined = ScopeCheckResult::Incomplete;
        }
        if exit == ScopeExit::Completed
            && !self.pending.is_empty()
            && combined == ScopeCheckResult::Accepting
        {
            combined = ScopeCheckResult::PendingEvidence;
        }
        self.record_exit(kind, combined);
        Ok(combined)
    }

    /// Exports this scope for an engine checkpoint.
    pub(crate) fn snapshot(&self) -> crate::orchestration::ResidualScopeSnapshot {
        use crate::orchestration::{
            ResidualEvidenceSnapshot, ResidualExitSnapshot, ResidualOutcomeSnapshot,
            ResidualOutcomeStatusSnapshot, ResidualPendingSnapshot,
        };
        crate::orchestration::ResidualScopeSnapshot {
            item: self.prepared.item.0,
            application: self.application.checked.fingerprint,
            revision: self.prepared.revision,
            invocation: *self.invocation.as_bytes(),
            version: self.version,
            last_cursor: self.last_cursor.map(|cursor| cursor.0),
            states: self
                .states
                .iter()
                .map(ResidualScopeState::to_data)
                .collect(),
            outcomes: self
                .outcomes
                .iter()
                .map(|outcome| ResidualOutcomeSnapshot {
                    action: outcome.action.clone(),
                    accepted: outcome.accepted.map(|accepted| {
                        let (attempt, completed_at) = accepted.history();
                        ResidualEvidenceSnapshot {
                            attempt: *attempt.as_bytes(),
                            completed_at: completed_at.0,
                        }
                    }),
                    rejected: outcome.rejected.map(|status| match status {
                        ResidualOutcomeStatus::Denied => ResidualOutcomeStatusSnapshot::Denied,
                        _ => ResidualOutcomeStatusSnapshot::Failed,
                    }),
                })
                .collect(),
            pending: self
                .pending
                .iter()
                .map(|pending| ResidualPendingSnapshot {
                    attempt: *pending.attempt.as_bytes(),
                    action: pending.action.clone(),
                })
                .collect(),
            exit: self.exit.map(|(kind, result)| ResidualExitSnapshot {
                kind: exit_kind_snapshot(kind),
                result: scope_result_snapshot(result),
            }),
        }
    }

    /// Restores a checkpointed scope through the freshly prepared plans of
    /// the recompiled program. Plan hashes, revision, component states and
    /// every action label are revalidated; nothing is defaulted.
    pub(crate) fn restore(
        snapshot: &crate::orchestration::ResidualScopeSnapshot,
        prepared: Arc<PreparedResidualScope>,
        program: CheckedProgram,
        log: &Rc<ResidualRunLog>,
        limits: ResidualStateLimits,
    ) -> Result<Self, String> {
        use crate::orchestration::ResidualOutcomeStatusSnapshot;
        if snapshot.item != prepared.item.0 {
            return Err("residual checkpoint scope belongs to a different callable".into());
        }
        if snapshot.revision != prepared.revision {
            return Err(
                "residual checkpoint scope was bound to a different checked revision".into(),
            );
        }
        if snapshot.states.len() != prepared.plans.len() {
            return Err("residual checkpoint scope does not cover every installed plan".into());
        }
        let application = prepared
            .applications
            .iter()
            .find(|application| application.checked.fingerprint == snapshot.application)
            .ok_or("residual checkpoint has no matching checked application")?
            .clone();
        let span = program
            .hir
            .items
            .get(prepared.item)
            .ok_or("residual checkpoint scope has no checked declaration")?
            .span();
        validate_component_budget(&prepared, limits.max_components)?;
        if snapshot.outcomes.len() > limits.max_outcomes
            || snapshot.pending.len() > limits.max_pending
        {
            return Err("residual checkpoint scope exceeds its state ceilings".into());
        }
        let invocation = ScopeInvocationId::from_bytes(snapshot.invocation);
        let mut states = Vec::with_capacity(prepared.plans.len());
        let mut attempt_cursor = None;
        for (index, (plan, data)) in prepared.plans.iter().zip(&snapshot.states).enumerate() {
            let scope = AlternativeScope {
                scope: plan.scope,
                activation: plan.activation,
                revision: prepared.revision,
                invocation,
            };
            let state = plan
                .checked
                .plan()
                .restore_specialized_scope(
                    scope,
                    data,
                    limits.max_components,
                    &application.scopes[index],
                )
                .map_err(|error| {
                    format!("residual checkpoint scope state is invalid: {error:?}")
                })?;
            attempt_cursor = attempt_cursor.max(data.last_cursor);
            states.push(state);
        }
        let mut instance = Self {
            prepared,
            application,
            program,
            invocation,
            span,
            states,
            labels: HashMap::new(),
            outcomes: Vec::new(),
            pending: Vec::new(),
            version: snapshot.version,
            last_cursor: snapshot.last_cursor.map(SemanticEventCursor),
            exit: None,
            log: Rc::downgrade(log),
            limits,
        };
        if attempt_cursor > snapshot.last_cursor {
            return Err("residual checkpoint attempt postdates its scope state".into());
        }
        if (snapshot.version == 0) != snapshot.last_cursor.is_none() {
            return Err("residual checkpoint scope version disagrees with its event cursor".into());
        }
        for outcome in &snapshot.outcomes {
            instance.validate_saved_action(&outcome.action)?;
            let action = outcome.action.clone();
            if instance.outcomes.iter().any(|entry| entry.action == action) {
                return Err("residual checkpoint scope repeats an outcome label".into());
            }
            if outcome.accepted.is_none() && outcome.rejected.is_none() {
                return Err("residual checkpoint outcome records no status".into());
            }
            if let Some(evidence) = outcome.accepted
                && (evidence.completed_at == 0
                    || Some(evidence.completed_at) > snapshot.last_cursor)
            {
                return Err("residual checkpoint evidence postdates its scope state".into());
            }
            instance.outcomes.push(OutcomeEntry {
                accepted: outcome.accepted.map(|evidence| {
                    let id = ActionAttemptId::from_bytes(evidence.attempt);
                    let cursor = SemanticEventCursor(evidence.completed_at);
                    if instance.prepared.is_approval(&action) {
                        AcceptedOutcome::Historical(id, cursor)
                    } else {
                        AcceptedOutcome::CurrentRun(id, cursor)
                    }
                }),
                action,
                rejected: outcome.rejected.map(|status| match status {
                    ResidualOutcomeStatusSnapshot::Denied => ResidualOutcomeStatus::Denied,
                    ResidualOutcomeStatusSnapshot::Failed => ResidualOutcomeStatus::Failed,
                }),
            });
        }
        for pending in &snapshot.pending {
            let attempt = ActionAttemptId::from_bytes(pending.attempt);
            if instance
                .pending
                .iter()
                .any(|entry| entry.attempt == attempt)
            {
                return Err("residual checkpoint scope repeats a pending attempt".into());
            }
            instance.validate_saved_action(&pending.action)?;
            let action = pending.action.clone();
            instance.pending.push(PendingEntry { attempt, action });
        }
        instance.exit = snapshot.exit.map(|exit| {
            (
                exit_kind_restore(exit.kind),
                scope_result_restore(exit.result),
            )
        });
        Ok(instance)
    }

    /// Highest event cursor recorded in this scope's committed state.
    pub(crate) fn cursor_floor(&self) -> u64 {
        self.last_cursor.map_or(0, |cursor| cursor.0)
    }

    fn validate_next_cursor(&self, cursor: SemanticEventCursor) -> Result<(), String> {
        if cursor.0 == 0 || self.last_cursor.is_some_and(|last| cursor <= last) {
            return Err("residual scope event cursor did not advance".into());
        }
        Ok(())
    }

    fn record_exit(&mut self, kind: ResidualExitKind, result: ScopeCheckResult) {
        self.exit = Some((kind, result));
        if let Some(log) = self.log.upgrade() {
            log.record(ResidualExitRecord {
                item: self.prepared.item,
                revision: self.prepared.revision,
                obligations: self.prepared.obligation_ids(),
                span: self.span,
                invocation: self.invocation,
                kind,
                result,
                unknown_outcomes: self.pending.len(),
            });
        }
    }
}

fn validate_component_budget(prepared: &PreparedResidualScope, limit: usize) -> Result<(), String> {
    let mut remaining = limit;
    for plan in &prepared.plans {
        for obligation in plan.checked.plan().obligations().iter() {
            let identity = obligation.identity();
            if identity.scope == plan.scope && identity.activation == plan.activation {
                remaining = remaining
                    .checked_sub(1)
                    .ok_or("residual scope exceeded its aggregate component ceiling")?;
            }
        }
    }
    Ok(())
}

impl Drop for ResidualScopeInstance {
    fn drop(&mut self) {
        // The last owner of a scope that never reached an exit path, such as
        // a continuation abandoned by a handler, records an abort.
        if self.exit.is_none() {
            self.record_exit(ResidualExitKind::Abort, ScopeCheckResult::Incomplete);
        }
    }
}

/// Bounded view over this scope's own correlated outcomes. Only an outcome
/// the engine classified as accepted after observing it in this scope
/// instance can satisfy a prior-evidence requirement.
struct ScopeEvidence<'a> {
    instance: &'a ResidualScopeInstance,
}

impl ConcretePriorEvidence for ScopeEvidence<'_> {
    fn query(&self, query: EvidenceQuery<'_, '_>) -> EvidenceStatus {
        let attempt = query.attempt;
        let mut accepted: Option<(ActionAttemptId, SemanticEventCursor)> = None;
        let mut denied = false;
        let mut failed = false;
        for outcome in &self.instance.outcomes {
            if !query.guard.covers(&outcome.action) {
                continue;
            }
            if let Some((id, completed_at)) = outcome.accepted.and_then(AcceptedOutcome::current)
                && completed_at < attempt.cursor
                && id != attempt.id
                && accepted.is_none_or(|(_, latest)| completed_at > latest)
            {
                accepted = Some((id, completed_at));
            }
            match outcome.rejected {
                Some(ResidualOutcomeStatus::Denied) => denied = true,
                Some(ResidualOutcomeStatus::Failed) => failed = true,
                _ => {}
            }
        }
        if let Some((guard_attempt, completed_at)) = accepted {
            return EvidenceStatus::Accepted(AcceptedPriorEvidence {
                scope: attempt.scope,
                activation: attempt.activation,
                revision: attempt.revision,
                invocation: attempt.invocation,
                guard_pattern: query.guard.id(),
                target_pattern: query.target.id(),
                guard_attempt,
                completed_at,
            });
        }
        if self
            .instance
            .pending
            .iter()
            .any(|pending| pending.attempt != attempt.id && query.guard.covers(&pending.action))
        {
            return EvidenceStatus::Pending;
        }
        if denied {
            EvidenceStatus::Denied
        } else if failed {
            EvidenceStatus::Failed
        } else {
            EvidenceStatus::Absent
        }
    }
}

pub(super) fn exit_kind_snapshot(
    kind: ResidualExitKind,
) -> crate::orchestration::ResidualExitKindSnapshot {
    use crate::orchestration::ResidualExitKindSnapshot as Snapshot;
    match kind {
        ResidualExitKind::Return => Snapshot::Return,
        ResidualExitKind::NonLocalReturn => Snapshot::NonLocalReturn,
        ResidualExitKind::ErrorPropagation => Snapshot::ErrorPropagation,
        ResidualExitKind::HandlerFinish => Snapshot::HandlerFinish,
        ResidualExitKind::Abort => Snapshot::Abort,
        ResidualExitKind::Cancellation => Snapshot::Cancellation,
        ResidualExitKind::Fault => Snapshot::Fault,
    }
}

pub(super) fn exit_kind_restore(
    kind: crate::orchestration::ResidualExitKindSnapshot,
) -> ResidualExitKind {
    use crate::orchestration::ResidualExitKindSnapshot as Snapshot;
    match kind {
        Snapshot::Return => ResidualExitKind::Return,
        Snapshot::NonLocalReturn => ResidualExitKind::NonLocalReturn,
        Snapshot::ErrorPropagation => ResidualExitKind::ErrorPropagation,
        Snapshot::HandlerFinish => ResidualExitKind::HandlerFinish,
        Snapshot::Abort => ResidualExitKind::Abort,
        Snapshot::Cancellation => ResidualExitKind::Cancellation,
        Snapshot::Fault => ResidualExitKind::Fault,
    }
}

pub(super) fn scope_result_snapshot(
    result: ScopeCheckResult,
) -> crate::orchestration::ResidualScopeResultSnapshot {
    use crate::orchestration::ResidualScopeResultSnapshot as Snapshot;
    match result {
        ScopeCheckResult::Accepting => Snapshot::Accepting,
        ScopeCheckResult::Rejected => Snapshot::Rejected,
        ScopeCheckResult::PendingEvidence => Snapshot::PendingEvidence,
        ScopeCheckResult::Incomplete => Snapshot::Incomplete,
    }
}

pub(super) fn scope_result_restore(
    result: crate::orchestration::ResidualScopeResultSnapshot,
) -> ScopeCheckResult {
    use crate::orchestration::ResidualScopeResultSnapshot as Snapshot;
    match result {
        Snapshot::Accepting => ScopeCheckResult::Accepting,
        Snapshot::Rejected => ScopeCheckResult::Rejected,
        Snapshot::PendingEvidence => ScopeCheckResult::PendingEvidence,
        Snapshot::Incomplete => ScopeCheckResult::Incomplete,
    }
}

//! Data-only checkpoint form of live residual monitor state.
//!
//! Every value here is untrusted until restored through the freshly prepared
//! plans of the recompiled program: plan hashes, revisions, component states,
//! action labels and attempt correlations are all revalidated, and missing
//! or forged state rejects the checkpoint instead of resetting monitors.

use etas_code::{RevisionKey, residual::plan::ResidualScopeStateData};

/// Upper bounds applied before any residual checkpoint data is restored.
pub(crate) const MAX_CHECKPOINT_RESIDUAL_SCOPES: usize = 1024;
pub(crate) const MAX_CHECKPOINT_RESIDUAL_ATTEMPTS: usize = 4096;
pub(crate) const MAX_CHECKPOINT_RESIDUAL_ENTRIES: usize = 4096;
pub(crate) const MAX_CHECKPOINT_RESIDUAL_EXITS: usize = 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidualMachineSnapshot {
    /// Run-scoped counters; restored counters never reuse a captured value.
    pub(crate) next_cursor: u64,
    pub(crate) next_attempt: u64,
    pub(crate) next_invocation: u64,
    pub(crate) scopes: Vec<ResidualScopeSnapshot>,
    pub(crate) attempts: Vec<ResidualAttemptSnapshot>,
    pub(crate) exits: ResidualExitLogSnapshot,
}

impl ResidualMachineSnapshot {
    pub(crate) fn is_empty(&self) -> bool {
        self.scopes.is_empty() && self.attempts.is_empty() && self.exits.unsatisfied == 0
    }

    pub(crate) fn validate_counters(&self) -> Result<(), String> {
        self.exits.validate(self.next_invocation)?;
        let cursor = |cursor: u64| {
            if cursor == 0 || cursor > self.next_cursor {
                Err("checkpoint residual cursor would reuse a recorded event".to_owned())
            } else {
                Ok(())
            }
        };
        let attempt = |id: &[u8; 16]| validate_counter_identity(id, b'a', self.next_attempt);
        let mut invocations = std::collections::BTreeSet::new();
        for scope in &self.scopes {
            if (scope.version == 0) != scope.last_cursor.is_none() {
                return Err(
                    "residual checkpoint scope version disagrees with its event cursor".into(),
                );
            }
            if scope
                .states
                .iter()
                .any(|state| state.last_cursor > scope.last_cursor)
            {
                return Err("residual checkpoint attempt postdates its scope state".into());
            }
            validate_counter_identity(&scope.invocation, b's', self.next_invocation)?;
            if !invocations.insert(scope.invocation) {
                return Err("checkpoint residual scopes share an invocation identity".into());
            }
            if let Some(last) = scope.last_cursor {
                cursor(last)?;
            }
            for outcome in &scope.outcomes {
                if let Some(evidence) = &outcome.accepted {
                    if Some(evidence.completed_at) > scope.last_cursor {
                        return Err("residual checkpoint evidence postdates its scope state".into());
                    }
                    attempt(&evidence.attempt)?;
                    cursor(evidence.completed_at)?;
                }
            }
            for pending in &scope.pending {
                attempt(&pending.attempt)?;
            }
        }
        let mut attempts = std::collections::BTreeSet::new();
        for saved in &self.attempts {
            attempt(&saved.id)?;
            cursor(saved.cursor)?;
            if !attempts.insert(saved.id) {
                return Err("checkpoint residual attempts share an identity".into());
            }
            if !saved.dispatched {
                return Err("checkpoint has an undispatched residual permit with no safe authorization point".into());
            }
            if saved.completed && !saved.dispatched {
                return Err("checkpoint residual outcome precedes its dispatch".into());
            }
        }
        Ok(())
    }
}

/// Non-accepting historical exits remain durable after their frame is gone.
/// These records cannot grant evidence or supply source spans from an artifact.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidualExitLogSnapshot {
    pub(crate) unsatisfied: u64,
    pub(crate) omitted_exits: u64,
    pub(crate) records: Vec<ResidualExitRecordSnapshot>,
}

impl ResidualExitLogSnapshot {
    pub(crate) fn validate(&self, next_invocation: u64) -> Result<(), String> {
        if self.records.len() > MAX_CHECKPOINT_RESIDUAL_EXITS
            || self.records.len() as u64 > self.unsatisfied
            || self.unsatisfied > next_invocation
            || self.omitted_exits > next_invocation
            || (self.unsatisfied == 0) != self.records.is_empty()
        {
            return Err("checkpoint residual exit history has invalid counts".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for record in &self.records {
            validate_counter_identity(&record.invocation, b's', next_invocation)?;
            if !seen.insert(record.invocation)
                || record.exit.result == ResidualScopeResultSnapshot::Accepting
                || record.unknown_outcomes > MAX_CHECKPOINT_RESIDUAL_ENTRIES
            {
                return Err("checkpoint residual exit history has an invalid record".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidualExitRecordSnapshot {
    pub(crate) item: u32,
    pub(crate) revision: RevisionKey,
    pub(crate) invocation: [u8; 16],
    pub(crate) exit: ResidualExitSnapshot,
    pub(crate) unknown_outcomes: usize,
}

fn validate_counter_identity(id: &[u8; 16], kind: u8, maximum: u64) -> Result<(), String> {
    let mut bytes = [0; 8];
    bytes.copy_from_slice(&id[8..]);
    let counter = u64::from_be_bytes(bytes);
    if id[0] != kind || id[1..8] != [0; 7] || counter == 0 {
        return Err("checkpoint residual identity is not a run-owned counter identity".into());
    }
    if counter > maximum {
        return Err("checkpoint residual counters would reuse an identity".into());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidualScopeSnapshot {
    pub(crate) item: u32,
    pub(crate) revision: RevisionKey,
    pub(crate) invocation: [u8; 16],
    pub(crate) version: u64,
    /// Includes both committed attempts and their correlated outcomes.
    pub(crate) last_cursor: Option<u64>,
    /// One state per prepared plan of the item, in preparation order.
    pub(crate) states: Vec<ResidualScopeStateData>,
    pub(crate) outcomes: Vec<ResidualOutcomeSnapshot>,
    pub(crate) pending: Vec<ResidualPendingSnapshot>,
    pub(crate) exit: Option<ResidualExitSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidualOutcomeSnapshot {
    pub(crate) action: etas_code::contract::CanonicalEffect,
    pub(crate) accepted: Option<ResidualEvidenceSnapshot>,
    pub(crate) rejected: Option<ResidualOutcomeStatusSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidualEvidenceSnapshot {
    pub(crate) attempt: [u8; 16],
    pub(crate) completed_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum ResidualOutcomeStatusSnapshot {
    Denied,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidualPendingSnapshot {
    pub(crate) attempt: [u8; 16],
    pub(crate) action: etas_code::contract::CanonicalEffect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidualExitSnapshot {
    pub(crate) kind: ResidualExitKindSnapshot,
    pub(crate) result: ResidualScopeResultSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum ResidualExitKindSnapshot {
    Return,
    NonLocalReturn,
    ErrorPropagation,
    HandlerFinish,
    Abort,
    Cancellation,
    Fault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum ResidualScopeResultSnapshot {
    Accepting,
    Rejected,
    PendingEvidence,
    Incomplete,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidualAttemptSnapshot {
    pub(crate) id: [u8; 16],
    pub(crate) cursor: u64,
    pub(crate) scopes: Vec<ResidualObservedScopeSnapshot>,
    pub(crate) dispatched: bool,
    pub(crate) completed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidualObservedScopeSnapshot {
    /// Index into `ResidualMachineSnapshot::scopes`.
    pub(crate) scope: u32,
    pub(crate) action: etas_code::contract::CanonicalEffect,
    pub(crate) version: u64,
}

use std::collections::BTreeMap;

use etas_types::TypeId;

use crate::{control::Continuation, value::ContinuationTokenValue};

use super::{ContinuationId, SchedulerError, TaskIdentity, TaskScopeId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContinuationState {
    ArmOwned,
    ReadyOwned,
    WaitingOn,
    Resumed,
    Disposed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContinuationTransfer {
    Resume,
    Requeue,
    Park,
}

pub(crate) struct ConsumedContinuation {
    pub(crate) continuation: Continuation,
    pub(crate) output: TypeId,
}

struct ContinuationEntry {
    origin: TaskIdentity,
    continuation: Option<Continuation>,
    output: TypeId,
    state: ContinuationState,
}

pub(super) struct ContinuationArena {
    owner: TaskScopeId,
    next: u64,
    entries: BTreeMap<ContinuationId, ContinuationEntry>,
}

impl ContinuationArena {
    pub(super) fn new(owner: TaskScopeId) -> Self {
        Self {
            owner,
            next: 0,
            entries: BTreeMap::new(),
        }
    }

    pub(super) fn allocate(
        &mut self,
        origin: TaskIdentity,
        continuation: Continuation,
        output: TypeId,
    ) -> ContinuationTokenValue {
        let id = ContinuationId(self.next);
        self.next = self.next.checked_add(1).expect("continuation id exhausted");
        self.entries.insert(
            id,
            ContinuationEntry {
                origin,
                continuation: Some(continuation),
                output,
                state: ContinuationState::ArmOwned,
            },
        );
        ContinuationTokenValue::new(self.owner, id)
    }

    pub(super) fn consume(
        &mut self,
        token: ContinuationTokenValue,
        origin: TaskIdentity,
        transfer: ContinuationTransfer,
    ) -> Result<ConsumedContinuation, SchedulerError> {
        if token.owner != self.owner {
            return Err(SchedulerError::WrongOwner);
        }
        let entry = self
            .entries
            .get_mut(&token.continuation)
            .ok_or(SchedulerError::UnknownContinuation)?;
        if entry.origin != origin {
            return Err(SchedulerError::WrongTask);
        }
        if entry.state != ContinuationState::ArmOwned {
            return Err(SchedulerError::ContinuationConsumed);
        }
        entry.state = match transfer {
            ContinuationTransfer::Resume => ContinuationState::Resumed,
            ContinuationTransfer::Requeue => ContinuationState::ReadyOwned,
            ContinuationTransfer::Park => ContinuationState::WaitingOn,
        };
        Ok(ConsumedContinuation {
            continuation: entry
                .continuation
                .take()
                .expect("arm-owned continuation must retain its runtime stack"),
            output: entry.output,
        })
    }

    pub(super) fn dispose(
        &mut self,
        token: ContinuationTokenValue,
        origin: TaskIdentity,
    ) -> Result<(), SchedulerError> {
        if token.owner != self.owner {
            return Err(SchedulerError::WrongOwner);
        }
        let entry = self
            .entries
            .get_mut(&token.continuation)
            .ok_or(SchedulerError::UnknownContinuation)?;
        if entry.origin != origin {
            return Err(SchedulerError::WrongTask);
        }
        if entry.state != ContinuationState::ArmOwned {
            return Err(SchedulerError::ContinuationConsumed);
        }
        entry.continuation = None;
        entry.state = ContinuationState::Disposed;
        Ok(())
    }
}

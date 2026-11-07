use std::collections::BTreeMap;

use etas_types::TypeId;

use crate::{control::Continuation, value::ContinuationTokenValue};

use super::{ContinuationId, SchedulerError, TaskIdentity, TaskScopeId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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
    binder: Option<etas_hir::SymbolId>,
    program: Option<crate::value::CheckedExecutionProgram>,
    owner: TaskScopeId,
    origin: TaskIdentity,
    continuation: Option<Continuation>,
    output: TypeId,
    state: ContinuationState,
}

pub(super) struct ContinuationArena {
    next: u64,
    entries: BTreeMap<ContinuationId, ContinuationEntry>,
}

/// Durable entries include consumed tokens so restoring a checkpoint cannot
/// revive a continuation whose stack was already transferred to a task.
#[derive(Clone, Debug, Default)]
pub(crate) struct ContinuationArenaSnapshot {
    pub(crate) next: u64,
    pub(crate) entries: BTreeMap<ContinuationId, ContinuationEntrySnapshot>,
}

#[derive(Clone, Debug)]
pub(crate) struct ContinuationEntrySnapshot {
    pub(crate) binder: Option<etas_hir::SymbolId>,
    pub(crate) program: Option<crate::orchestration::ExecutionProgramSnapshot>,
    pub(crate) owner: TaskScopeId,
    pub(crate) origin: TaskIdentity,
    pub(crate) continuation: Option<u32>,
    pub(crate) output: TypeId,
    pub(crate) state: ContinuationState,
}

impl ContinuationArena {
    pub(super) fn capture(
        &self,
        revisions: &mut std::collections::BTreeMap<
            etas_code::RevisionKey,
            std::sync::Arc<crate::api::code::PreparedCodeRevision>,
        >,
        mut capture: impl FnMut(
            TaskIdentity,
            Option<&crate::value::CheckedExecutionProgram>,
            &Continuation,
        ) -> Result<u32, String>,
    ) -> Result<ContinuationArenaSnapshot, String> {
        let entries = self
            .entries
            .iter()
            .map(|(id, entry)| {
                Ok((
                    *id,
                    ContinuationEntrySnapshot {
                        binder: entry.binder,
                        program: entry
                            .program
                            .as_ref()
                            .map(|program| program.checkpoint_identity(revisions)),
                        owner: entry.owner,
                        origin: entry.origin,
                        continuation: entry
                            .continuation
                            .as_ref()
                            .map(|continuation| {
                                capture(entry.origin, entry.program.as_ref(), continuation)
                            })
                            .transpose()?,
                        output: entry.output,
                        state: entry.state,
                    },
                ))
            })
            .collect::<Result<_, String>>()?;
        Ok(ContinuationArenaSnapshot {
            next: self.next,
            entries,
        })
    }

    pub(super) fn restore(
        snapshot: ContinuationArenaSnapshot,
        mut restore: impl FnMut(
            TaskIdentity,
            Option<&crate::orchestration::ExecutionProgramSnapshot>,
            Option<u32>,
        ) -> Result<
            (
                Option<Continuation>,
                Option<crate::value::CheckedExecutionProgram>,
            ),
            String,
        >,
    ) -> Result<Self, String> {
        snapshot.validate()?;
        let entries = snapshot
            .entries
            .into_iter()
            .map(|(id, entry)| {
                let (continuation, program) =
                    restore(entry.origin, entry.program.as_ref(), entry.continuation)?;
                if let Some(binder) = entry.binder {
                    if !program.as_ref().is_some_and(|program| {
                        program
                            .checked
                            .types
                            .continuation_bindings
                            .contains_key(&binder)
                    }) {
                        return Err(
                            "continuation binder has no checked Handler ownership fact".into()
                        );
                    }
                }
                Ok((
                    id,
                    ContinuationEntry {
                        binder: entry.binder,
                        program,
                        owner: entry.owner,
                        origin: entry.origin,
                        continuation,
                        output: entry.output,
                        state: entry.state,
                    },
                ))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            next: snapshot.next,
            entries,
        })
    }

    pub(super) fn new() -> Self {
        Self {
            next: 0,
            entries: BTreeMap::new(),
        }
    }

    pub(super) fn dispose_origin(&mut self, origin: TaskIdentity) {
        for entry in self
            .entries
            .values_mut()
            .filter(|entry| entry.origin == origin)
        {
            if entry.state == ContinuationState::ArmOwned {
                entry.continuation = None;
                entry.state = ContinuationState::Disposed;
            }
        }
    }

    pub(super) fn allocate(
        &mut self,
        owner: TaskScopeId,
        origin: TaskIdentity,
        continuation: Continuation,
        output: TypeId,
        program: Option<crate::value::CheckedExecutionProgram>,
        binder: Option<etas_hir::SymbolId>,
    ) -> ContinuationTokenValue {
        let id = ContinuationId(self.next);
        self.next = self.next.checked_add(1).expect("continuation id exhausted");
        self.entries.insert(
            id,
            ContinuationEntry {
                binder,
                program,
                owner,
                origin,
                continuation: Some(continuation),
                output,
                state: ContinuationState::ArmOwned,
            },
        );
        ContinuationTokenValue::new(owner, id)
    }

    pub(super) fn consume(
        &mut self,
        token: ContinuationTokenValue,
        origin: TaskIdentity,
        transfer: ContinuationTransfer,
    ) -> Result<ConsumedContinuation, SchedulerError> {
        let entry = self
            .entries
            .get_mut(&token.continuation)
            .ok_or(SchedulerError::UnknownContinuation)?;
        if token.owner != entry.owner {
            return Err(SchedulerError::WrongOwner);
        }
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
        let entry = self
            .entries
            .get_mut(&token.continuation)
            .ok_or(SchedulerError::UnknownContinuation)?;
        if token.owner != entry.owner {
            return Err(SchedulerError::WrongOwner);
        }
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

impl ContinuationArenaSnapshot {
    pub(crate) fn validate(&self) -> Result<(), String> {
        for (id, entry) in &self.entries {
            if id.0 >= self.next {
                return Err("checkpoint continuation counter would reuse a token".into());
            }
            if (entry.state == ContinuationState::ArmOwned) != entry.continuation.is_some() {
                return Err(
                    "checkpoint continuation ownership differs from its saved stack".into(),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        control::Frame,
        eval::machine::snapshot::{CaptureContext, RestoreContext},
        value::InterpValue,
    };
    use etas_hir::{HirBlockId, SymbolId};

    #[test]
    fn checkpoint_roundtrip_preserves_consumption_ids_deferred_tokens_and_shared_frames() {
        let owner = TaskScopeId(7);
        let origin = TaskIdentity::Scheduled(super::super::TaskId(9));
        let mut arena = ContinuationArena::new();
        let mut frame = Frame::from_snapshot(vec![(SymbolId(0), InterpValue::i32(3))]).unwrap();
        let leaf = Continuation::ContinueBlock {
            block: HirBlockId(0),
            next_stmt_index: 1,
            frame: frame.clone(),
        };
        let resumed = arena.allocate(owner, origin, Continuation::Return, TypeId(0), None, None);
        let requeued = arena.allocate(owner, origin, Continuation::Return, TypeId(0), None, None);
        let parked = arena.allocate(owner, origin, leaf.clone(), TypeId(0), None, None);
        let disposed = arena.allocate(owner, origin, Continuation::Return, TypeId(0), None, None);
        for (token, transfer) in [
            (resumed, ContinuationTransfer::Resume),
            (requeued, ContinuationTransfer::Requeue),
            (parked, ContinuationTransfer::Park),
        ] {
            arena.consume(token, origin, transfer).unwrap();
        }
        arena.dispose(disposed, origin).unwrap();
        let first = arena.allocate(owner, origin, leaf, TypeId(0), None, None);
        let second = arena.allocate(
            owner,
            origin,
            Continuation::HandlerDispatch {
                outer: Continuation::ContinueBlock {
                    block: HirBlockId(0),
                    next_stmt_index: 2,
                    frame: frame.clone(),
                }
                .into(),
                deferred: Some(parked),
            },
            TypeId(0),
            None,
            None,
        );
        let mut capture = CaptureContext::default();
        capture.task_handles = true;
        let mut continuations = Vec::new();
        let saved = arena
            .capture(&mut Default::default(), |_, _, continuation| {
                let index = continuations.len() as u32;
                continuations.push(capture.continuation(continuation)?);
                Ok(index)
            })
            .unwrap();
        assert_eq!(
            saved.entries[&resumed.continuation].state,
            ContinuationState::Resumed
        );
        assert_eq!(
            saved.entries[&requeued.continuation].state,
            ContinuationState::ReadyOwned
        );
        assert_eq!(
            saved.entries[&parked.continuation].state,
            ContinuationState::WaitingOn
        );
        assert_eq!(
            saved.entries[&disposed.continuation].state,
            ContinuationState::Disposed
        );
        frame.insert(SymbolId(0), InterpValue::i32(99));
        let (saved, continuations) =
            crate::api::codec::checkpoint::scheduler::roundtrip_continuation_arena(
                &saved,
                &continuations,
            );
        let mut context = RestoreContext::default();
        let mut restored = ContinuationArena::restore(saved, |_, _, index| {
            Ok((
                index
                    .map(|index| {
                        continuations[index as usize]
                            .clone()
                            .restore_with(&mut context)
                    })
                    .transpose()?,
                None,
            ))
        })
        .unwrap();
        for token in [resumed, requeued, parked, disposed] {
            assert!(matches!(
                restored.consume(token, origin, ContinuationTransfer::Resume),
                Err(SchedulerError::ContinuationConsumed)
            ));
        }
        let a = restored
            .consume(first, origin, ContinuationTransfer::Resume)
            .unwrap()
            .continuation;
        let b = restored
            .consume(second, origin, ContinuationTransfer::Resume)
            .unwrap()
            .continuation;
        let Continuation::ContinueBlock { frame: mut a, .. } = a else {
            panic!("first stack")
        };
        let Continuation::HandlerDispatch { outer, deferred } = b else {
            panic!("second stack")
        };
        assert_eq!(deferred, Some(parked));
        let Continuation::ContinueBlock { frame: b, .. } = &*outer else {
            panic!("shared frame")
        };
        assert_eq!(a.get(SymbolId(0)), Some(InterpValue::i32(3)));
        a.insert(SymbolId(0), InterpValue::i32(42));
        assert_eq!(b.get(SymbolId(0)), Some(InterpValue::i32(42)));
        let fresh = restored.allocate(owner, origin, Continuation::Return, TypeId(0), None, None);
        assert_eq!(fresh.continuation.0, second.continuation.0 + 1);
    }
}

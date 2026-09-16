use std::{
    collections::{HashMap, HashSet},
    ptr::NonNull,
    rc::Rc,
};

use super::capture_identity::CaptureIdentity;
use crate::{
    control::{CallTarget, Continuation, Frame},
    orchestration::{
        CallTargetSnapshotChildren, CallTargetSnapshotLink, ContinuationSnapshotLink,
        LocalsSnapshot, ValueSnapshot,
    },
    value::InterpValue,
};

#[cfg(test)]
#[path = "capture_context_tests.rs"]
mod tests;

type FrameIdentity = (*const (), u64);

/// One synchronous, read-only capture operation. All keyed runtime nodes remain
/// owned by the borrowed root, including its captured frames. No cache escapes.
#[derive(Default)]
pub(in crate::eval::machine) struct CaptureContext {
    pub(super) values: HashMap<CaptureIdentity, ValueSnapshot>,
    pub(super) call_nodes: HashMap<*const CallTarget, CallTargetSnapshotLink>,
    pub(super) call_tables: HashMap<*const Vec<CallTarget>, CallTargetSnapshotChildren>,
    pub(super) continuations: HashMap<NonNull<Continuation>, ContinuationSnapshotLink>,
    first_frame: Option<(FrameIdentity, LocalsSnapshot)>,
    frames: HashMap<FrameIdentity, LocalsSnapshot>,
    first_active: Option<*const ()>,
    active_frames: HashSet<*const ()>,
}

impl CaptureContext {
    pub(super) fn call_target(
        &mut self,
        target: &CallTarget,
    ) -> Result<crate::orchestration::CallTargetSnapshot, String> {
        super::call_target::capture_call_target_with(target, self)
    }

    pub(super) fn continuation(
        &mut self,
        continuation: &crate::control::Continuation,
    ) -> Result<crate::orchestration::ContinuationSnapshot, String> {
        crate::orchestration::ContinuationSnapshot::capture_with(continuation, self)
    }

    pub(super) fn value(&mut self, value: &InterpValue) -> Result<ValueSnapshot, String> {
        super::value_capture::capture_with(value, self)
    }

    pub(super) fn frame(&mut self, frame: &Frame) -> Result<LocalsSnapshot, String> {
        let identity = frame.shared_capture_identity();
        if let Some(saved) = identity.and_then(|key| self.saved_frame(key)) {
            return Ok(saved.clone());
        }
        if let Some((backing, _)) = identity {
            if !self.enter_frame(backing) {
                return Err("cyclic captured frame definitions cannot enter a checkpoint".into());
            }
        }
        let result = frame
            .try_map_locals(|value| self.value(value))
            .map(|locals| LocalsSnapshot {
                id: frame.snapshot_id(),
                locals: Rc::new(locals),
                type_bindings: frame.sorted_type_bindings(),
            });
        if let Some(key @ (backing, _)) = identity {
            if self.first_active == Some(backing) {
                self.first_active = None;
            } else {
                self.active_frames.remove(&backing);
            }
            if let Ok(saved) = &result {
                if self.first_frame.is_none() {
                    self.first_frame = Some((key, saved.clone()));
                } else {
                    self.frames.insert(key, saved.clone());
                }
            }
        }
        result
    }

    // A single captured frame needs neither a lookup table nor an active-set
    // allocation. Nested/shared graphs still use constant-time indexed lookup.
    fn saved_frame(&self, key: FrameIdentity) -> Option<&LocalsSnapshot> {
        if let Some((first_key, saved)) = &self.first_frame {
            if *first_key == key {
                return Some(saved);
            }
        }
        self.frames.get(&key)
    }

    fn enter_frame(&mut self, backing: *const ()) -> bool {
        match self.first_active {
            Some(first) if first == backing => false,
            Some(_) => self.active_frames.insert(backing),
            None => {
                self.first_active = Some(backing);
                true
            }
        }
    }
}

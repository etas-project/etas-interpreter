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
pub(crate) struct CaptureContext {
    pub(crate) revisions: std::collections::BTreeMap<
        etas_code::RevisionKey,
        std::sync::Arc<crate::api::code::PreparedCodeRevision>,
    >,
    pub(super) values: HashMap<CaptureIdentity, ValueSnapshot>,
    pub(super) call_nodes: HashMap<*const CallTarget, CallTargetSnapshotLink>,
    pub(super) call_tables: HashMap<*const Vec<CallTarget>, CallTargetSnapshotChildren>,
    pub(super) continuations: HashMap<NonNull<Continuation>, ContinuationSnapshotLink>,
    suspension_ids: HashMap<usize, u32>,
    action_providers: HashMap<u32, usize>,
    pending_suspensions: Vec<super::super::suspension::ProgramContinuation>,
    first_frame: Option<(FrameIdentity, LocalsSnapshot)>,
    frames: HashMap<FrameIdentity, LocalsSnapshot>,
    first_active: Option<*const ()>,
    active_frames: HashSet<*const ()>,
    residual_scope_ids: HashMap<*const (), u32>,
    residual_scopes: Vec<crate::control::residual::SharedResidualScope>,
    residual_attempt_ids: HashMap<*const (), u32>,
    residual_attempts: Vec<crate::control::residual::ResidualAttemptTag>,
    pub(crate) task_handles: bool,
}

impl CaptureContext {
    pub(super) fn suspension(
        &mut self,
        continuation: &super::super::suspension::ProgramContinuation,
    ) -> Result<u32, String> {
        let identity = continuation.checkpoint_identity()?;
        if let Some(id) = self.suspension_ids.get(&identity) {
            return Ok(*id);
        }
        let id = u32::try_from(self.pending_suspensions.len())
            .ok()
            .and_then(|id| id.checked_add(1))
            .ok_or("too many suspended program segments in checkpoint")?;
        self.suspension_ids.insert(identity, id);
        self.action_providers
            .insert(id, continuation.projected_action()?.evidence_identity()?);
        self.pending_suspensions.push(continuation.clone());
        Ok(id)
    }

    pub(in crate::eval::machine) fn action_origin(
        &mut self,
        projection: &crate::plan::code::ActionProjection,
        continuation: &crate::orchestration::ContinuationSnapshot,
    ) -> Result<crate::orchestration::ActionInvocationSnapshot, String> {
        let key = projection.source_identity()?;
        let mut found = None;
        for id in continuation.suspension_ids() {
            if self.action_providers.get(&id) == Some(&key) {
                if found.is_some() {
                    return Err("action invocation has multiple suspended origin owners".into());
                }
                found = Some(id);
            }
        }
        match found {
            Some(id) => Ok(crate::orchestration::ActionInvocationSnapshot::FromSuspension { id }),
            None => projection.checkpoint_origin(&mut self.revisions),
        }
    }

    /// Registers a live scope once per capture, preserving shared identity.
    pub(in crate::eval::machine) fn residual_scope(
        &mut self,
        scope: &crate::control::residual::SharedResidualScope,
    ) -> Result<u32, String> {
        let identity = Rc::as_ptr(scope).cast::<()>();
        if let Some(id) = self.residual_scope_ids.get(&identity) {
            return Ok(*id);
        }
        let id = u32::try_from(self.residual_scopes.len())
            .ok()
            .filter(|id| (*id as usize) < crate::orchestration::MAX_CHECKPOINT_RESIDUAL_SCOPES)
            .ok_or("too many residual scopes in checkpoint")?;
        self.residual_scope_ids.insert(identity, id);
        self.residual_scopes.push(scope.clone());
        Ok(id)
    }

    /// Registers an in-flight attempt and every scope that observed it.
    pub(in crate::eval::machine) fn residual_attempt(
        &mut self,
        tag: &crate::control::residual::ResidualAttemptTag,
    ) -> Result<u32, String> {
        let identity = tag.identity();
        if let Some(id) = self.residual_attempt_ids.get(&identity) {
            return Ok(*id);
        }
        for scope in tag.observing_scopes() {
            self.residual_scope(scope)?;
        }
        let id = u32::try_from(self.residual_attempts.len())
            .ok()
            .filter(|id| (*id as usize) < crate::orchestration::MAX_CHECKPOINT_RESIDUAL_ATTEMPTS)
            .ok_or("too many residual attempts in checkpoint")?;
        self.residual_attempt_ids.insert(identity, id);
        self.residual_attempts.push(tag.clone());
        Ok(id)
    }

    /// Captures every registered residual scope and attempt together with
    /// the run counters, after all frames and continuations were captured.
    pub(in crate::eval::machine) fn capture_residual(
        &self,
        root: Option<&etas_frontend::CheckedProgram>,
        counters: (u64, u64, u64),
    ) -> Result<crate::orchestration::ResidualMachineSnapshot, String> {
        let mut scopes = Vec::with_capacity(self.residual_scopes.len());
        for scope in &self.residual_scopes {
            let scope = scope
                .try_borrow()
                .map_err(|_| "residual scope is busy during checkpoint capture".to_owned())?;
            if !root.is_some_and(|root| std::sync::Arc::ptr_eq(root, scope.program())) {
                return Err(
                    "residual scope of a compiled revision cannot cross a checkpoint boundary"
                        .into(),
                );
            }
            scopes.push(scope.snapshot());
        }
        let attempts = self
            .residual_attempts
            .iter()
            .map(|tag| {
                tag.snapshot(|scope| {
                    self.residual_scope_ids
                        .get(&Rc::as_ptr(scope).cast::<()>())
                        .copied()
                        .ok_or_else(|| "residual attempt scope was not captured".to_owned())
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(crate::orchestration::ResidualMachineSnapshot {
            next_cursor: counters.0,
            next_attempt: counters.1,
            next_invocation: counters.2,
            scopes,
            attempts,
            exits: Default::default(),
        })
    }

    pub(in crate::eval::machine) fn capture_suspensions(
        &mut self,
    ) -> Result<Vec<crate::orchestration::SuspendedProgramSnapshot>, String> {
        let mut result = Vec::new();
        while result.len() < self.pending_suspensions.len() {
            let pending = self.pending_suspensions[result.len()].clone();
            result.push(pending.capture(self)?);
        }
        Ok(result)
    }
    pub(super) fn call_target(
        &mut self,
        target: &CallTarget,
    ) -> Result<crate::orchestration::CallTargetSnapshot, String> {
        super::call_target::capture_call_target_with(target, self)
    }

    pub(crate) fn continuation(
        &mut self,
        continuation: &crate::control::Continuation,
    ) -> Result<crate::orchestration::ContinuationSnapshot, String> {
        crate::orchestration::ContinuationSnapshot::capture_with(continuation, self)
    }

    pub(crate) fn value(&mut self, value: &InterpValue) -> Result<ValueSnapshot, String> {
        super::value_capture::capture_with(value, self)
    }

    pub(super) fn frame(&mut self, frame: &Frame) -> Result<LocalsSnapshot, String> {
        let identity = frame.capture_identity();
        if let Some(saved) = self.saved_frame(identity) {
            return Ok(saved.clone());
        }
        if self.first_active.is_some() {
            return Err("cyclic captured frame definitions cannot enter a checkpoint".into());
        }
        let dependencies = self.frame_dependencies(frame)?;
        if dependencies.is_empty() {
            return self.capture_ready_frame(frame);
        }
        let mut active = HashSet::new();
        active.insert(identity.0);
        let mut pending = vec![(frame.clone(), true)];
        pending.extend(dependencies.into_iter().rev());
        while let Some((frame, complete)) = pending.pop() {
            let key = frame.capture_identity();
            if complete {
                self.capture_ready_frame(&frame)?;
                active.remove(&key.0);
            } else if self.saved_frame(key).is_none() {
                if !active.insert(key.0) {
                    return Err(
                        "cyclic captured frame definitions cannot enter a checkpoint".into(),
                    );
                }
                let dependencies = self.frame_dependencies(&frame)?;
                pending.push((frame, true));
                pending.extend(dependencies.into_iter().rev());
            }
        }
        self.saved_frame(identity)
            .cloned()
            .ok_or_else(|| "captured frame dependency graph did not produce its root".into())
    }

    fn frame_dependencies(&self, frame: &Frame) -> Result<Vec<(Frame, bool)>, String> {
        let mut discovery = super::frame_dependencies::Discovery::default();
        let mut result = Vec::new();
        frame.try_visit_locals(|value| {
            result.extend(
                discovery
                    .frames(crate::value::graph::Node::Value(value), true, |key| {
                        self.values.contains_key(key)
                    })
                    .into_iter()
                    .map(|frame| (frame, false)),
            );
            Ok::<_, String>(())
        })?;
        Ok(result)
    }

    fn capture_ready_frame(&mut self, frame: &Frame) -> Result<LocalsSnapshot, String> {
        let key @ (backing, _) = frame.capture_identity();
        if !self.enter_frame(backing) {
            return Err("cyclic captured frame definitions cannot enter a checkpoint".into());
        }
        let result = frame
            .try_map_locals(|value| self.value(value))
            .map(|locals| LocalsSnapshot {
                residual_invocation: frame.residual_invocation(),
                id: frame.snapshot_id(),
                locals: Rc::new(locals),
                type_bindings: frame.sorted_type_bindings(),
                type_parameter_bindings: frame.generic_bindings().sorted_parameters(),
                effect_row_bindings: frame.generic_bindings().sorted_rows(),
            });
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

use std::{collections::HashMap, ptr::NonNull, rc::Rc};

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

#[derive(Clone, Default)]
pub(crate) struct SharedFrameCapture {
    frames: Rc<std::cell::RefCell<HashMap<FrameIdentity, LocalsSnapshot>>>,
    cycles: Rc<std::cell::Cell<bool>>,
}

/// One synchronous, read-only capture operation. All keyed runtime nodes remain
/// owned by the borrowed root, including its captured frames. No cache escapes.
#[derive(Default)]
pub(crate) struct CaptureContext {
    programs: HashMap<usize, crate::orchestration::CheckpointProgramReference>,
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
    pending_suspensions: Vec<super::super::suspension::SuspendedExecution>,
    first_frame: Option<(FrameIdentity, LocalsSnapshot)>,
    frames: HashMap<FrameIdentity, LocalsSnapshot>,
    first_active: Option<(FrameIdentity, usize)>,
    active_frames: HashMap<FrameIdentity, usize>,
    capture_depth: usize,
    has_frame_cycles: bool,
    residual_scope_ids: HashMap<*const (), u32>,
    residual_scopes: Vec<crate::control::residual::SharedResidualScope>,
    residual_attempt_ids: HashMap<*const (), u32>,
    residual_attempts: Vec<crate::control::residual::ResidualAttemptTag>,
    pub(crate) task_handles: bool,
    shared_frames: Option<SharedFrameCapture>,
}

impl CaptureContext {
    fn register_program(
        &mut self,
        program: &etas_frontend::CheckedProgram,
        identity: crate::orchestration::CheckpointProgramReference,
    ) {
        let key = std::sync::Arc::as_ptr(program) as usize;
        self.programs.entry(key).or_insert_with(|| identity.clone());
        if let Some(library) = &program.standard_library {
            self.programs
                .entry(std::sync::Arc::as_ptr(&library.program) as usize)
                .or_insert(
                    crate::orchestration::CheckpointProgramReference::StandardLibrary {
                        owner: Box::new(identity),
                    },
                );
        }
    }

    pub(crate) fn bind_revision(
        &mut self,
        revision: &std::sync::Arc<crate::api::code::PreparedCodeRevision>,
    ) {
        self.revisions
            .insert(revision.revision().key(), revision.clone());
        self.register_program(
            revision.type_bridge().original_program(),
            crate::orchestration::CheckpointProgramReference::Source,
        );
        self.register_program(
            revision.program(),
            crate::orchestration::CheckpointProgramReference::Revision {
                revision: Box::new(crate::orchestration::CheckpointCodeRevision::for_revision(
                    revision,
                )),
            },
        );
    }

    pub(crate) fn bind_program(&mut self, program: &crate::value::CheckedExecutionProgram) {
        if let Some(revision) = &program.revision {
            self.bind_revision(revision);
        } else if self.programs.is_empty() {
            self.register_program(
                &program.checked,
                crate::orchestration::CheckpointProgramReference::Source,
            );
        }
    }

    pub(crate) fn program_reference(
        &self,
        program: &etas_frontend::CheckedProgram,
    ) -> Result<crate::orchestration::CheckpointProgramReference, String> {
        self.programs
            .get(&(std::sync::Arc::as_ptr(program) as usize))
            .cloned()
            .ok_or_else(|| "checkpoint program has no checked source, revision or std owner".into())
    }

    pub(crate) fn execution_identity(
        &mut self,
        program: &crate::value::CheckedExecutionProgram,
    ) -> Result<crate::orchestration::ExecutionProgramSnapshot, String> {
        self.bind_program(program);
        let mut identity = program.checkpoint_identity(&mut self.revisions);
        identity.program = self.program_reference(&program.checked)?;
        Ok(identity)
    }

    pub(crate) fn for_tasks(frames: &SharedFrameCapture) -> Self {
        Self {
            task_handles: true,
            shared_frames: Some(frames.clone()),
            ..Default::default()
        }
    }
    pub(super) fn suspension(
        &mut self,
        continuation: &super::super::suspension::ProgramContinuation,
    ) -> Result<u32, String> {
        self.suspended_execution(super::super::suspension::SuspendedExecution::Program(
            continuation.clone(),
        ))
    }

    pub(super) fn task_suspension(
        &mut self,
        continuation: &super::super::TaskHandlerContinuation,
    ) -> Result<u32, String> {
        self.suspended_execution(super::super::suspension::SuspendedExecution::Task(
            continuation.clone(),
        ))
    }

    fn suspended_execution(
        &mut self,
        continuation: super::super::suspension::SuspendedExecution,
    ) -> Result<u32, String> {
        let identity = continuation.identity()?;
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
        self.pending_suspensions.push(continuation);
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
            None => projection.checkpoint_origin(self),
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
    ) -> Result<Vec<crate::orchestration::SuspendedExecutionSnapshot>, String> {
        let mut result = Vec::new();
        while result.len() < self.pending_suspensions.len() {
            let pending = self.pending_suspensions[result.len()].clone();
            result.push(pending.capture(self)?);
        }
        Ok(result)
    }
    pub(crate) fn call_target(
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
        self.has_frame_cycles |= self
            .shared_frames
            .as_ref()
            .is_some_and(|shared| shared.cycles.get());
        if let Some(depth) = self.active_depth(identity) {
            return if self.capture_depth > depth {
                self.mark_frame_cycle();
                Ok(LocalsSnapshot::reference(frame.snapshot_id()))
            } else {
                Err("cyclic captured frame definitions cannot enter a checkpoint".into())
            };
        }
        if let Some(saved) = self.saved_frame(identity) {
            return Ok(saved);
        }
        let depth = self.capture_depth;
        let dependencies = self.frame_dependencies(frame, depth)?;
        if dependencies.is_empty() {
            self.enter_frame(identity, depth);
            let result = self.capture_ready_frame(frame, depth);
            self.leave_frame(identity);
            return result;
        }
        let mut pending = vec![(frame.clone(), false, self.capture_depth)];
        let mut entered = Vec::new();
        let result = (|| {
            while let Some((frame, complete, depth)) = pending.pop() {
                let key = frame.capture_identity();
                if complete {
                    self.capture_ready_frame(&frame, depth)?;
                    self.leave_frame(key);
                } else if self.saved_frame(key).is_none() {
                    if let Some(previous) = self.active_depth(key) {
                        if depth <= previous {
                            return Err(
                                "cyclic captured frame definitions cannot enter a checkpoint"
                                    .into(),
                            );
                        }
                        self.mark_frame_cycle();
                        continue;
                    }
                    self.enter_frame(key, depth);
                    entered.push(key);
                    let dependencies = self.frame_dependencies(&frame, depth)?;
                    pending.push((frame, true, depth));
                    pending.extend(dependencies.into_iter().rev());
                }
            }
            self.saved_frame(identity)
                .ok_or_else(|| "captured frame dependency graph did not produce its root".into())
        })();
        for key in entered {
            self.leave_frame(key);
        }
        if self.has_frame_cycles {
            self.clear_frame_value_caches();
        }
        result
    }

    fn frame_dependencies(
        &self,
        frame: &Frame,
        depth: usize,
    ) -> Result<Vec<(Frame, bool, usize)>, String> {
        let mut discovery = super::frame_dependencies::Discovery::default();
        let mut result = Vec::new();
        if let Some(parent) = frame.lexical_parent() {
            result.push((parent.clone(), false, depth));
        }
        let bindings = frame.shared_bindings();
        if bindings.is_empty() {
            frame.try_visit_locals(|value| {
                result.extend(
                    discovery
                        .frames(crate::value::graph::Node::Value(value), true, |key| {
                            !self.has_frame_cycles && self.values.contains_key(key)
                        })
                        .into_iter()
                        .map(|frame| (frame, false, depth)),
                );
                Ok::<_, String>(())
            })?;
            return Ok(result);
        }
        let locals = frame.borrow_locals()?;
        for (symbol, value) in locals.sorted() {
            let depth =
                depth + usize::from(bindings.iter().any(|binding| binding.symbol == symbol));
            result.extend(
                discovery
                    .frames(crate::value::graph::Node::Value(value), true, |key| {
                        !self.has_frame_cycles && self.values.contains_key(key)
                    })
                    .into_iter()
                    .map(|frame| (frame, false, depth)),
            );
        }
        Ok(result)
    }

    fn capture_ready_frame(
        &mut self,
        frame: &Frame,
        depth: usize,
    ) -> Result<LocalsSnapshot, String> {
        let key = frame.capture_identity();
        if self.has_frame_cycles {
            self.clear_frame_value_caches();
        }
        let activations = frame
            .handler_activations()
            .into_iter()
            .map(|activation| {
                Ok(crate::orchestration::HandlerActivationSnapshot {
                    arm: activation.arm,
                    request: activation.request.checkpoint_evidence(self)?,
                    types: activation.bindings.sorted_types(),
                    parameters: activation.bindings.sorted_parameters(),
                    rows: activation.bindings.sorted_rows(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let previous_depth = self.capture_depth;
        let locals = frame.try_map_locals(|_, shared, value| {
            self.capture_depth = depth + usize::from(shared);
            self.value(value)
        });
        self.capture_depth = previous_depth;
        let saved = LocalsSnapshot {
            reference: false,
            shared_bindings: frame.shared_bindings(),
            parent: frame
                .lexical_parent()
                .map(|parent| {
                    self.saved_frame(parent.capture_identity())
                        .expect("lexical parent captured before child")
                })
                .map(Rc::new),
            handler_activations: activations,
            residual_invocation: frame.residual_invocation(),
            id: frame.snapshot_id(),
            locals: Rc::new(locals?),
            type_bindings: frame.sorted_type_bindings(),
            type_parameter_bindings: frame.generic_bindings().sorted_parameters(),
            effect_row_bindings: frame.generic_bindings().sorted_rows(),
        };
        if let Some(shared) = &self.shared_frames {
            shared.frames.borrow_mut().insert(key, saved.clone());
        }
        if self
            .first_frame
            .as_ref()
            .is_none_or(|(first, _)| *first == key)
        {
            self.first_frame = Some((key, saved.clone()));
        } else {
            self.frames.insert(key, saved.clone());
        }
        Ok(saved)
    }

    // A single captured frame needs neither a lookup table nor an active-set
    // allocation. Nested/shared graphs still use constant-time indexed lookup.
    fn saved_frame(&self, key: FrameIdentity) -> Option<LocalsSnapshot> {
        let saved = self
            .first_frame
            .as_ref()
            .filter(|(first, _)| *first == key)
            .map(|(_, frame)| frame.clone())
            .or_else(|| self.frames.get(&key).cloned())
            .or_else(|| {
                self.shared_frames
                    .as_ref()
                    .and_then(|shared| shared.frames.borrow().get(&key).cloned())
            })?;
        if self.has_frame_cycles && !self.reusable_frame(&saved) {
            return None;
        }
        Some(saved)
    }

    // An inner definition can refer back to its former capture root. Re-root
    // that graph before reusing it outside the ancestor's active traversal.
    fn reusable_frame(&self, root: &LocalsSnapshot) -> bool {
        let active = |id| {
            self.first_active.is_some_and(|((_, frame), _)| frame == id)
                || self.active_frames.keys().any(|(_, frame)| *frame == id)
        };
        let mut definitions = std::collections::HashSet::new();
        let mut references = Vec::new();
        let mut pending = vec![(root.clone(), false)];
        while let Some((frame, _)) = pending.pop() {
            if frame.reference {
                references.push(frame.id);
                continue;
            }
            if active(frame.id) {
                return false;
            }
            if !definitions.insert(frame.id) {
                continue;
            }
            if let Some(parent) = &frame.parent {
                pending.push(((**parent).clone(), false));
            }
            for (_, value) in frame.locals.iter() {
                super::frame_dependencies::snapshot_frames(value, &mut pending);
            }
        }
        references
            .into_iter()
            .all(|id| definitions.contains(&id) || active(id))
    }

    fn mark_frame_cycle(&mut self) {
        self.has_frame_cycles = true;
        if let Some(shared) = &self.shared_frames {
            shared.cycles.set(true);
        }
    }

    fn clear_frame_value_caches(&mut self) {
        self.values.clear();
        self.call_nodes.clear();
        self.call_tables.clear();
    }

    fn active_depth(&self, key: FrameIdentity) -> Option<usize> {
        self.first_active
            .filter(|(first, _)| *first == key)
            .map(|(_, depth)| depth)
            .or_else(|| self.active_frames.get(&key).copied())
    }

    fn enter_frame(&mut self, key: FrameIdentity, depth: usize) {
        if self.first_active.is_none() {
            self.first_active = Some((key, depth));
        } else {
            self.active_frames.insert(key, depth);
        }
    }

    fn leave_frame(&mut self, key: FrameIdentity) {
        if self.first_active.is_some_and(|(first, _)| first == key) {
            self.first_active = None;
        } else {
            self.active_frames.remove(&key);
        }
    }
}

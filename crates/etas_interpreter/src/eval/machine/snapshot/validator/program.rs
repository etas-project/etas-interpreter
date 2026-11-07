use super::*;

struct Boundary {
    index: usize,
    floor: usize,
    program: crate::value::CheckedExecutionProgram,
}

impl SnapshotValidator<'_> {
    pub(super) fn validate_auxiliary(&self, snapshot: &MachineSnapshot) -> Result<(), String> {
        for entry in &snapshot.auxiliary {
            let mut scopes = BTreeSet::new();
            if matches!(
                entry.program.program,
                crate::orchestration::CheckpointProgramReference::Source
            ) && self.execution_revision.is_none()
            {
                self.continuation(&entry.continuation, "task continuation", &mut scopes)?;
                continue;
            }
            let program = self
                .revisions
                .ok_or("task continuation requires prepared revisions")?
                .execution_snapshot(&entry.program)?;
            let mut validator = SnapshotValidator::new(
                &program.checked,
                &program.plan.slots,
                &program.plan.dispatch,
                &program.plan.closures,
                self.limits,
            )
            .with_revisions(self.revisions);
            validator.execution_revision = program.revision.as_deref();
            validator.tasks = self.tasks;
            validator.task_identity = self.task_identity;
            validator.frame_definitions = self.frame_definitions.clone();
            validator.frame_owners = self.frame_owners.clone();
            validator.suspension_references = self.suspension_references.clone();
            validator.residual_references = self.residual_references.clone();
            validator.continuation(&entry.continuation, "task continuation", &mut scopes)?;
        }
        Ok(())
    }

    pub(super) fn active_program(
        &self,
        snapshot: &MachineSnapshot,
    ) -> Result<Option<crate::value::CheckedExecutionProgram>, String> {
        Ok(self
            .program_boundaries(snapshot)?
            .last()
            .map(|boundary| boundary.program.clone()))
    }
    fn program_boundaries(&self, snapshot: &MachineSnapshot) -> Result<Vec<Boundary>, String> {
        snapshot.validate_program_owners()?;
        let mut boundaries: Vec<Boundary> = Vec::new();
        let mut floor = 0;
        for (index, frame) in snapshot.frames.iter().enumerate() {
            if let MachineFrameSnapshot::ProgramReturn(frame) = frame {
                let parent = boundaries
                    .last()
                    .map_or(self.checked, |b| &b.program.checked);
                let program = super::super::program::resolve_destination(
                    frame,
                    parent,
                    self.revisions,
                    self.limits,
                )?;
                if frame.handler_floor < floor {
                    return Err("checkpoint program handler floors are not nested".into());
                }
                floor = frame.handler_floor;
                boundaries.push(Boundary {
                    index,
                    floor,
                    program,
                });
            }
        }
        Ok(boundaries)
    }

    pub(super) fn validate_program_frames(
        &self,
        snapshot: &MachineSnapshot,
        scopes: &mut BTreeSet<HandlerScopeId>,
    ) -> Result<(), String> {
        let boundaries = self.program_boundaries(snapshot)?;
        let root_end = boundaries
            .first()
            .map_or(snapshot.frames.len(), |b| b.index + 1);
        self.validate_local_frames(&snapshot.frames[..root_end], scopes)?;
        self.validate_residual_frame_coverage(
            &snapshot.frames[..root_end],
            &snapshot.residual,
            None,
        )?;
        for (position, boundary) in boundaries.iter().enumerate() {
            if boundary.floor != scopes.len() + self.inherited_handlers {
                return Err(
                    "checkpoint program handler floor disagrees with its parent boundaries".into(),
                );
            }
            let end = boundaries
                .get(position + 1)
                .map_or(snapshot.frames.len(), |b| b.index + 1);
            let mut validator = SnapshotValidator::new(
                &boundary.program.checked,
                &boundary.program.plan.slots,
                &boundary.program.plan.dispatch,
                &boundary.program.plan.closures,
                self.limits,
            )
            .with_revisions(self.revisions)
            .with_execution_revision(boundary.program.revision.as_deref());
            validator.frame_owners = self.frame_owners.clone();
            validator.frame_definitions = self.frame_definitions.clone();
            validator.tasks = self.tasks;
            validator.task_identity = self.task_identity;
            validator.suspension_references = self.suspension_references.clone();
            validator.residual_references = self.residual_references.clone();
            validator.validate_local_frames(&snapshot.frames[boundary.index + 1..end], scopes)?;
            validator.validate_residual_frame_coverage(
                &snapshot.frames[boundary.index + 1..end],
                &snapshot.residual,
                None,
            )?;
        }
        Ok(())
    }

    pub(super) fn validate_program_handlers(
        &self,
        checkpoint: &InterpreterCheckpoint,
    ) -> Result<Vec<HandlerScopeId>, String> {
        self.validate_handler_stack(
            &checkpoint.machine,
            &checkpoint.handlers,
            checkpoint.retry_state.attempts.len(),
            0,
        )
    }

    pub(super) fn validate_handler_stack(
        &self,
        machine: &MachineSnapshot,
        handlers: &HandlerSnapshot,
        retries: usize,
        floor: usize,
    ) -> Result<Vec<HandlerScopeId>, String> {
        let boundaries = self.program_boundaries(machine)?;
        let count = handlers.handlers.len();
        if floor > count
            || boundaries
                .iter()
                .any(|b| b.floor < floor || b.floor > count)
        {
            return Err("checkpoint program handler floor exceeds active handler stack".into());
        }
        let mut retry_depth = 0;
        for frame in &machine.frames {
            if let MachineFrameSnapshot::ProgramReturn(frame) = frame {
                if frame.retry_depth < retry_depth || frame.retry_depth > retries {
                    return Err(
                        "checkpoint program retry depths disagree with active retry stack".into(),
                    );
                }
                retry_depth = frame.retry_depth;
            }
        }
        let root_end = boundaries.first().map_or(count, |b| b.floor);
        let mut scopes = self.handlers(&HandlerSnapshot {
            handlers: handlers.handlers[floor..root_end].to_vec(),
        })?;
        for (index, boundary) in boundaries.iter().enumerate() {
            let end = boundaries.get(index + 1).map_or(count, |b| b.floor);
            let validator = SnapshotValidator::new(
                &boundary.program.checked,
                &boundary.program.plan.slots,
                &boundary.program.plan.dispatch,
                &boundary.program.plan.closures,
                self.limits,
            )
            .with_revisions(self.revisions);
            scopes.extend(validator.handlers(&HandlerSnapshot {
                handlers: handlers.handlers[boundary.floor..end].to_vec(),
            })?);
        }
        if scopes.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err("checkpoint handler scopes are not globally ordered".into());
        }
        Ok(scopes)
    }
}

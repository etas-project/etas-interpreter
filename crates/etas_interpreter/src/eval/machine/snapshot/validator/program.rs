use super::*;

struct Boundary {
    index: usize,
    floor: usize,
    program: crate::value::CheckedExecutionProgram,
}

impl SnapshotValidator<'_> {
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
        for (position, boundary) in boundaries.iter().enumerate() {
            if boundary.floor != scopes.len() {
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
            .with_revisions(self.revisions);
            validator.frame_owners = self.frame_owners.clone();
            validator.frame_definitions = self.frame_definitions.clone();
            validator.suspension_references = self.suspension_references.clone();
            validator.validate_local_frames(&snapshot.frames[boundary.index + 1..end], scopes)?;
        }
        Ok(())
    }

    pub(super) fn validate_program_handlers(
        &self,
        checkpoint: &InterpreterCheckpoint,
    ) -> Result<Vec<HandlerScopeId>, String> {
        let boundaries = self.program_boundaries(&checkpoint.machine)?;
        let count = checkpoint.handlers.handlers.len();
        if boundaries.iter().any(|b| b.floor > count) {
            return Err("checkpoint program handler floor exceeds active handler stack".into());
        }
        let mut retry_depth = 0;
        for frame in &checkpoint.machine.frames {
            if let MachineFrameSnapshot::ProgramReturn(frame) = frame {
                if frame.retry_depth < retry_depth
                    || frame.retry_depth > checkpoint.retry_state.attempts.len()
                {
                    return Err(
                        "checkpoint program retry depths disagree with active retry stack".into(),
                    );
                }
                retry_depth = frame.retry_depth;
            }
        }
        let root_end = boundaries.first().map_or(count, |b| b.floor);
        let mut scopes = self.handlers(&HandlerSnapshot {
            handlers: checkpoint.handlers.handlers[..root_end].to_vec(),
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
                handlers: checkpoint.handlers.handlers[boundary.floor..end].to_vec(),
            })?);
        }
        if scopes.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err("checkpoint handler scopes are not globally ordered".into());
        }
        Ok(scopes)
    }
}

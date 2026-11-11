use super::*;
use crate::orchestration::{
    ContinuationInputSnapshot as C, ControlSignalSnapshot as S, MachineInputSnapshot as I,
    TaskControlSnapshot as T,
};

impl SnapshotValidator<'_> {
    pub(super) fn validate_machine_input(&self, snapshot: &MachineSnapshot) -> Result<(), String> {
        let Some(input) = snapshot.input.as_deref() else {
            return Ok(());
        };
        if let I::EnterProgram { call, .. } = input {
            let boundary = snapshot
                .frames
                .iter()
                .rev()
                .find_map(|frame| match frame {
                    MachineFrameSnapshot::ProgramReturn(boundary) => Some(boundary),
                    _ => None,
                })
                .ok_or("pending program entry has no owning call boundary")?;
            if boundary.call != *call {
                return Err("pending program entry differs from its checked call boundary".into());
            }
            return Ok(());
        }
        if let Some(program) = self.active_program(snapshot)? {
            let mut validator = SnapshotValidator::new(
                &program.checked,
                &program.plan.slots,
                &program.plan.dispatch,
                &program.plan.closures,
                self.limits,
            )
            .with_revisions(self.revisions)
            .with_tasks(self.tasks, self.task_identity)
            .with_execution_revision(program.revision.as_deref());
            validator.frame_definitions = self.frame_definitions.clone();
            validator.frame_owners = self.frame_owners.clone();
            validator.suspension_references = self.suspension_references.clone();
            validator.residual_references = self.residual_references.clone();
            validator.input(input)
        } else {
            self.input(input)
        }
    }

    fn input(&self, input: &I) -> Result<(), String> {
        let Some(signal) = input.signal() else {
            return Ok(());
        };
        if let Some(continuation) = input.continuation_root() {
            self.continuation(
                continuation,
                "checkpoint machine input",
                &mut BTreeSet::new(),
            )?;
        }
        match signal {
            S::Error {
                error_type,
                payload,
                residual,
                ..
            } => {
                if let Some(attempt) = residual {
                    self.residual_references
                        .borrow_mut()
                        .attempts
                        .push(*attempt);
                }
                self.reference_value(
                    payload,
                    *error_type,
                    &crate::control::GenericBindings::new(),
                )
            }
            S::Input(input) | S::Apply { input, .. } => self.input_value(input),
            S::Expr { expr, frame, .. } => {
                self.expr(*expr, "checkpoint pending expression")?;
                self.frame(frame, "checkpoint pending expression")
            }
            S::Block {
                agent_context,
                block,
                next_stmt_index,
                frame,
                residual,
                ..
            } => {
                if let Some(request) = agent_context {
                    self.agent_request_context(request)?;
                }
                self.block(*block, "checkpoint pending block")?;
                if *next_stmt_index > self.checked.hir.blocks.get(*block).unwrap().stmts.len() {
                    return Err(
                        "checkpoint pending block cursor is outside its checked body".into(),
                    );
                }
                if let Some(residual) = residual {
                    self.item(residual.item, "checkpoint pending residual scope")?;
                }
                self.frame(frame, "checkpoint pending block")
            }
            S::Call {
                site, target, args, ..
            } => {
                self.call_target(target, "checkpoint pending call")?;
                for value in args {
                    self.snapshot_value_in(value, call_target::ReferenceScope::CheckedSlot)?;
                }
                if let Some(site) = site {
                    self.frame(&site.frame, "checkpoint pending call site")?;
                    if let Some(composition) = &site.composition {
                        self.call_target(&composition.root, "checkpoint pending composition")?;
                    }
                }
                Ok(())
            }
            S::Task(task) => {
                use crate::orchestration::TaskValueSnapshot as V;
                let scheduler = self
                    .tasks
                    .ok_or("pending task control has no scheduler snapshot")?;
                match task {
                    T::Run { scope, .. } => {
                        scheduler.validate_value(V::Scheduler { owner: *scope })
                    }
                    T::Park {
                        owner, task, token, ..
                    } => {
                        scheduler.validate_value(V::Task {
                            owner: *owner,
                            task: *task,
                        })?;
                        scheduler.validate_deferred(
                            *token,
                            self.task_identity
                                .ok_or("pending park has no task identity")?,
                        )
                    }
                    T::Requeue { token, value, .. } => {
                        scheduler.validate_deferred(
                            *token,
                            self.task_identity
                                .ok_or("pending requeue has no task identity")?,
                        )?;
                        self.snapshot_value_in(value, call_target::ReferenceScope::CheckedSlot)
                    }
                }
            }
            S::Checkpoint { .. } | S::Fault(_) => Ok(()),
        }
    }
    fn input_value(&self, input: &C) -> Result<(), String> {
        match input {
            C::Value(value) | C::Return(value) | C::Resume(value) | C::Finish(value) => {
                self.snapshot_value_in(value, call_target::ReferenceScope::CheckedSlot)
            }
            C::Break | C::Continue => Ok(()),
        }
    }
}

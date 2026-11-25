use super::*;
use crate::{driver::scheduler::TaskId, orchestration::CheckpointProgramReference as P};

impl SnapshotValidator<'_> {
    pub(crate) fn validate_task_local(
        &self,
        task: &crate::orchestration::ScheduledTaskSnapshot,
    ) -> Result<(), String> {
        task.local.model_policy.validate()?;
        if let Some(request) = &task.local.agent_context {
            self.agent_request_context(request)?;
        }
        if task.state == crate::orchestration::ScheduledTaskStateSnapshot::WaitingChildren
            && (task.outcome.is_none()
                || task.machine.is_some()
                || task.active_slot
                || task.waiting_continuation.is_some())
        {
            return Err("task waiting for children has no retained body outcome".into());
        }
        let Some(machine) = &task.machine else {
            return Ok(());
        };
        let (handlers, retries) = task.inherited_dynamic;
        if handlers > task.local.handlers.handlers.len()
            || retries > task.local.retries.attempts.len()
            || task.handler_floor > handlers
        {
            return Err("task dynamic scope depths exceed its inherited state".into());
        }
        self.validate_handler_stack(
            machine,
            &task.local.handlers,
            task.local.retries.attempts.len(),
            task.handler_floor,
        )?;
        let expected = task.local.handlers.handlers[handlers..]
            .iter()
            .rev()
            .map(|handler| handler.id)
            .collect::<Vec<_>>();
        if let Some(crate::orchestration::TaskResumeSnapshot::DeferredHandler { handler }) =
            &task.waiting_continuation
        {
            use crate::orchestration::{
                ControlSignalSnapshot, MachineInputSnapshot, ScheduledTaskStateSnapshot,
            };
            if !matches!(
                task.state,
                ScheduledTaskStateSnapshot::WaitingHandlerOwner(_)
                    | ScheduledTaskStateSnapshot::Ready
            ) || !task.local.handlers.handlers[..handlers]
                .iter()
                .any(|record| record.id == *handler)
                || !matches!(machine.input.as_deref(), Some(MachineInputSnapshot::Signal(signal)) if matches!(signal.as_ref(), ControlSignalSnapshot::HandlerRequest { .. }))
            {
                return Err(
                    "deferred Handler request has no matching task state, inherited scope or input"
                        .into(),
                );
            }
        }
        let mut found = Vec::new();
        let mut attempts = Vec::new();
        let auxiliary = task
            .waiting_continuation
            .as_ref()
            .and_then(|resume| match resume {
                crate::orchestration::TaskResumeSnapshot::DeferredHandler { .. } => None,
                crate::orchestration::TaskResumeSnapshot::Await(index)
                | crate::orchestration::TaskResumeSnapshot::Run(index)
                | crate::orchestration::TaskResumeSnapshot::Handler {
                    continuation: index,
                    ..
                }
                | crate::orchestration::TaskResumeSnapshot::Completion {
                    continuation: index,
                    ..
                }
                | crate::orchestration::TaskResumeSnapshot::Transfer {
                    continuation: index,
                    ..
                } => Some(*index),
            });
        if let Some(index) = auxiliary {
            let pending = machine
                .auxiliary
                .get(index as usize)
                .ok_or("task wait has no captured continuation")?;
            if let Some(crate::orchestration::TaskResumeSnapshot::Completion {
                result_type, ..
            }) = &task.waiting_continuation
            {
                if let Some(revisions) = self.revisions {
                    let program = revisions.execution_snapshot(&pending.program)?;
                    Self::completion_type_for(&program.checked, *result_type)?;
                } else if pending.program.program == P::Source {
                    self.completion_type(*result_type)?;
                } else {
                    return Err("completion wait has no checked program environment".into());
                }
            }
            Self::continuation_unwind_order(&pending.continuation, &mut found, &mut attempts);
        }
        if let Some(pending) = machine
            .input
            .as_deref()
            .and_then(|input| input.continuation_root())
        {
            Self::continuation_unwind_order(pending, &mut found, &mut attempts);
        }
        Self::frame_unwind_order(&machine.frames, &mut found, &mut attempts);
        if expected != found
            || attempts
                != task.local.retries.attempts[retries..]
                    .iter()
                    .rev()
                    .cloned()
                    .collect::<Vec<_>>()
        {
            return Err(
                "task Handler/retry stack differs from its saved machine continuation".into(),
            );
        }
        Ok(())
    }

    pub(super) fn completion_type(&self, ty: TypeId) -> Result<(), String> {
        Self::completion_type_for(self.checked, ty)
    }

    fn completion_type_for(
        checked: &etas_frontend::CheckedProject,
        ty: TypeId,
    ) -> Result<(), String> {
        let found = checked.hir.items.iter().any(|(item, declaration)| {
            matches!(
                declaration,
                etas_hir::HirItem::Flow(etas_hir::HirFlowDecl {
                    body: etas_hir::HirFlowBody::Runtime {
                        role: etas_std::StdExecutionRole::TaskNextCompletion,
                        ..
                    },
                    ..
                })
            ) && matches!(checked.types.item_signatures.get(&item),
                Some(etas_types::ItemSignature::Flow(signature)) if signature.output == ty)
        });
        if found {
            Ok(())
        } else {
            Err("completion receipt type differs from its checked source declaration".into())
        }
    }

    pub(super) fn task_output_type(&self, id: TaskId) -> Result<TypeId, String> {
        let saved = self
            .tasks
            .and_then(|tasks| tasks.task_types.get(&id))
            .ok_or("checkpoint Task handle has no checked start signature")?;
        let program = match &saved.program {
            P::Source if self.execution_revision.is_none() => self.checked.clone(),
            reference => self
                .revisions
                .ok_or("Task signature has no checked program environment")?
                .checked_program(reference)?,
        };
        let Some(task) = etas_types::source_execution::task(
            &program.type_store,
            &program.types.known_std_types,
            saved.ty,
        ) else {
            return Err("checkpoint Task signature does not return a Task".into());
        };
        let output = task.output;
        if std::sync::Arc::ptr_eq(&program, self.checked) {
            return Ok(output);
        }
        self.revisions
            .ok_or("Task output requires its checked program environment")?
            .project_type(&saved.program, self.checked, output)
    }
}

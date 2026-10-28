use super::*;
use crate::{
    eval::machine::snapshot::CaptureContext,
    orchestration::{
        HandlerSnapshot, MachineFrameSnapshot, RetrySnapshot, SuspendedProgramSnapshot,
    },
};

impl ProgramContinuation {
    pub(in crate::eval::machine) fn capture(
        &self,
        context: &mut CaptureContext,
    ) -> Result<SuspendedProgramSnapshot, String> {
        let state = self
            .0
            .as_ref()
            .ok_or("program continuation has no live identity")?
            .try_borrow()
            .map_err(|_| "program continuation is already being consumed")?;
        let saved = state
            .as_ref()
            .ok_or("program continuation has already been consumed")?;
        let Some(EvalFrame::ProgramReturn(boundary)) = saved.frames.first() else {
            return Err("suspended program has no owning call boundary".into());
        };
        let destination = boundary.call.destination()?;
        if !Arc::ptr_eq(&destination.checked, &saved.scope.program.checked)
            || !Arc::ptr_eq(
                &boundary.parent_program.checked,
                &saved.resuming_program.checked,
            )
            || boundary.parent_program.plan.root != saved.resuming_program.plan.root
            || !matches!(boundary.outer, Continuation::BlockValue)
        {
            return Err("suspended program differs from its checked boundary owners".into());
        }
        let count = saved
            .frames
            .iter()
            .filter(|frame| matches!(frame, EvalFrame::Call(_) | EvalFrame::ProgramReturn(_)))
            .count();
        if count != saved.depth.calls as usize {
            return Err("suspended program call-depth charge differs from its frames".into());
        }
        let boundary_snapshot =
            crate::eval::machine::snapshot::program::capture(boundary, &saved.scope, context)?;
        let mut frames = Vec::with_capacity(saved.frames.len() - 1);
        for frame in &saved.frames[1..] {
            if matches!(frame, EvalFrame::ProgramReturn(_)) {
                return Err("suspended segment contains another program boundary".into());
            }
            frames.push(MachineFrameSnapshot::capture(frame, context)?);
        }
        let continuation = context.continuation(&saved.continuation)?;
        let action = context.action_origin(&saved.projection, &continuation)?;
        Ok(SuspendedProgramSnapshot {
            parent: saved
                .resuming_program
                .checkpoint_identity(&mut context.revisions),
            parent_scoped: boundary.parent.is_some(),
            parent_handler_floor: boundary
                .parent
                .as_ref()
                .map_or(0, |scope| scope.handler_floor),
            boundary: Box::new(boundary_snapshot),
            frames,
            continuation,
            handlers: HandlerSnapshot {
                handlers: saved.handlers.clone(),
            },
            retries: RetrySnapshot {
                attempts: saved.retries.clone(),
            },
            model_policy: crate::eval::machine::snapshot::program::capture_model_policy(
                &saved.model_policy,
            ),
            action,
            span: saved.span,
        })
    }
}

fn context<'a>(
    contexts: &'a mut std::collections::HashMap<
        usize,
        crate::eval::machine::snapshot::RestoreContext,
    >,
    program: &crate::value::CheckedExecutionProgram,
    revisions: &Arc<crate::api::code::PreparedRevisionSet>,
    table: &crate::eval::machine::snapshot::SuspensionRestoreTable,
    locked_model: &Option<etas_host::ModelName>,
) -> &'a mut crate::eval::machine::snapshot::RestoreContext {
    contexts
        .entry(Arc::as_ptr(&program.checked) as usize)
        .or_insert_with(|| {
            let mut context = crate::eval::machine::snapshot::RestoreContext::for_program(
                program.checked.clone(),
                Some(revisions.clone()),
            );
            context.suspensions = Some(table.clone());
            context.locked_model = locked_model.clone();
            context
        })
}

pub(in crate::eval::machine) fn restore_graph(
    snapshot: &crate::orchestration::MachineSnapshot,
    revisions: &Arc<crate::api::code::PreparedRevisionSet>,
    contexts: &mut std::collections::HashMap<usize, crate::eval::machine::snapshot::RestoreContext>,
    table: &crate::eval::machine::snapshot::SuspensionRestoreTable,
    depth: &Rc<Cell<u32>>,
    current: &crate::api::HostExecutionContext,
    locked_model: &Option<etas_host::ModelName>,
) -> Result<(), String> {
    use crate::eval::machine::{frame::ProgramReturnFrame, scoped_call::ProgramCall};
    for index in snapshot.suspension_order()? {
        let saved = &snapshot.suspended_programs[index];
        let parent = revisions.execution_snapshot(&saved.parent)?;
        let call = context(contexts, &parent, revisions, table, locked_model)
            .restore_call_target(saved.boundary.call.clone())?;
        let call = ProgramCall::restore_target(call, &parent.checked)?;
        let program = call.destination()?;
        let proof = match &saved.action {
            crate::orchestration::ActionInvocationSnapshot::Native { .. } => {
                crate::control::CheckedActionInvocation::restore_origin(&saved.action, revisions)?
            }
            crate::orchestration::ActionInvocationSnapshot::FromSuspension { id } => table
                .borrow()
                .get(id)
                .ok_or("forwarded action source has not been restored")?
                .projected_action()?,
        };
        if !Arc::ptr_eq(&proof.program, &program.checked) {
            return Err(
                "suspended action origin differs from its checked native destination".into(),
            );
        }
        let projection = ActionProjection::new(proof, call.action_projections()?, &parent)?;
        let mut frames = Vec::with_capacity(saved.frames.len() + 1);
        frames.push(EvalFrame::ProgramReturn(Box::new(ProgramReturnFrame {
            site: Some(saved.boundary.call_site.clone().restore(context(
                contexts,
                &parent,
                revisions,
                table,
                locked_model,
            ))?),
            parent: saved.parent_scoped.then(|| ProgramScope {
                program: parent.clone(),
                handler_floor: saved.parent_handler_floor,
            }),
            parent_program: parent.clone(),
            parent_model_policy: crate::eval::machine::snapshot::program::restore_model_policy(
                saved.boundary.parent_model_policy.clone(),
            ),
            call,
            outer: Continuation::BlockValue,
            span: saved.boundary.span,
            retry_depth: saved.boundary.retry_depth,
        })));
        let native_context = context(contexts, &program, revisions, table, locked_model);
        for frame in saved.frames.iter().cloned() {
            frames.push(frame.restore_local(current, native_context)?);
        }
        let continuation = saved.continuation.clone().restore_with(native_context)?;
        let calls = u32::try_from(
            frames
                .iter()
                .filter(|frame| matches!(frame, EvalFrame::Call(_) | EvalFrame::ProgramReturn(_)))
                .count(),
        )
        .map_err(|_| "suspended call-depth charge overflow")?;
        let total = depth
            .get()
            .checked_add(calls)
            .ok_or("suspended call-depth charge overflow")?;
        depth.set(total);
        let restored = ProgramContinuation::new(SuspendedProgram {
            scope: ProgramScope {
                program,
                handler_floor: saved.boundary.handler_floor,
            },
            frames,
            continuation,
            handlers: saved.handlers.handlers.clone(),
            retries: saved.retries.attempts.clone(),
            model_policy: crate::eval::machine::snapshot::program::restore_model_policy(
                saved.model_policy.clone(),
            ),
            projection,
            resuming_program: parent,
            depth: CallDepthLease {
                total: depth.clone(),
                calls,
            },
            span: saved.span,
        });
        if table
            .borrow_mut()
            .insert(
                u32::try_from(index + 1).map_err(|_| "suspended graph identity overflow")?,
                restored,
            )
            .is_some()
        {
            return Err("suspended graph contains a duplicate identity".into());
        }
    }
    Ok(())
}

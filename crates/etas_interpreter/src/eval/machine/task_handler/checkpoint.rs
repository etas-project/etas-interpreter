use super::*;
use crate::{
    eval::machine::snapshot::{CaptureContext, program},
    orchestration::{
        HandlerSnapshot, MachineFrameSnapshot, RetrySnapshot, SuspendedTaskHandlerSnapshot,
        TaskHandlerWaitSnapshot,
    },
};

impl TaskHandlerContinuation {
    pub(in crate::eval::machine) fn checkpoint_identity(&self) -> Result<usize, String> {
        let state = self
            .0
            .try_borrow()
            .map_err(|_| "task Handler continuation is being consumed")?;
        if state.is_none() {
            return Err("task Handler continuation was already consumed".into());
        }
        Ok(Rc::as_ptr(&self.0) as usize)
    }

    pub(in crate::eval::machine) fn projected_action(
        &self,
    ) -> Result<crate::control::CheckedActionInvocation, String> {
        let state = self
            .0
            .try_borrow()
            .map_err(|_| "task Handler continuation is being consumed")?;
        Ok(state
            .as_ref()
            .ok_or("task Handler continuation was already consumed")?
            .projection
            .target()
            .clone())
    }

    pub(crate) fn capture(
        &self,
        context: &mut CaptureContext,
    ) -> Result<SuspendedTaskHandlerSnapshot, String> {
        let state = self
            .0
            .try_borrow()
            .map_err(|_| "task Handler continuation is being consumed")?;
        let saved = state
            .as_ref()
            .ok_or("task Handler continuation was already consumed")?;
        let owner = context.execution_identity(&saved.owner)?;
        if let Some(scope) = &saved.scope {
            context.bind_program(&scope.program);
        }
        for frame in &saved.frames {
            if let EvalFrame::ProgramReturn(boundary) = frame {
                context.bind_program(&boundary.parent_program);
                context.bind_program(&boundary.call.destination()?);
            }
        }
        let scopes = program::capture_segment_scopes(
            &saved.frames,
            saved.scope.clone(),
            Some(&saved.owner),
            saved.owner_scope.as_ref(),
        )?;
        let mut frames = Vec::with_capacity(saved.frames.len());
        for (index, frame) in saved.frames.iter().enumerate() {
            let snapshot = match frame {
                EvalFrame::ProgramReturn(boundary) => {
                    let mut boundary = program::capture(boundary, &scopes[&index], context)?;
                    boundary.handler_floor -= saved.handler_base;
                    boundary.retry_depth -= saved.retry_base;
                    MachineFrameSnapshot::ProgramReturn(Box::new(boundary))
                }
                frame => MachineFrameSnapshot::capture(frame, context)?,
            };
            frames.push(snapshot);
        }
        let wait = match &saved.wait {
            RootTaskWait::Ready(input) => {
                let parent = saved
                    .frames
                    .iter()
                    .rev()
                    .find_map(|frame| match frame {
                        EvalFrame::ProgramReturn(boundary) => Some(&boundary.parent_program),
                        _ => None,
                    })
                    .or_else(|| saved.scope.as_ref().map(|scope| &scope.program))
                    .unwrap_or(&saved.owner);
                let mut machine = EvalMachine::new();
                machine.restore_task_input((**input).clone());
                TaskHandlerWaitSnapshot::Ready(Box::new(
                    machine
                        .capture_input(context, Some(parent))?
                        .expect("saved ready wait has an input"),
                ))
            }
            RootTaskWait::Await {
                target,
                continuation,
            } => TaskHandlerWaitSnapshot::Await {
                target: *target,
                continuation: context.continuation(continuation)?,
            },
            RootTaskWait::Run { scope } => TaskHandlerWaitSnapshot::Run { scope: *scope },
            RootTaskWait::Transfer {
                target,
                token,
                continuation,
                output,
                span,
            } => TaskHandlerWaitSnapshot::Transfer {
                target: *target,
                token: *token,
                continuation: context.continuation(continuation)?,
                output: *output,
                span: *span,
            },
            RootTaskWait::Host => {
                return Err("suspended Host wait must be committed before task checkpoint".into());
            }
            RootTaskWait::Settling(_) => {
                return Err("task Handler cannot suspend a completed body".into());
            }
        };
        Ok(SuspendedTaskHandlerSnapshot {
            child: saved.child,
            owner_task: saved.owner_task,
            handler: saved.handler,
            owner,
            owner_scoped: saved.owner_scope.is_some(),
            owner_handler_floor: saved.owner_handler_floor,
            handler_base: saved.handler_base,
            retry_base: saved.retry_base,
            active_scoped: saved.scope.is_some(),
            frames,
            wait,
            handlers: HandlerSnapshot {
                handlers: saved.handlers.clone(),
            },
            retries: RetrySnapshot {
                attempts: saved.retries.clone(),
            },
            model_policy: program::capture_model_policy(&saved.model_policy),
            projection: saved.projection.checkpoint(context)?,
            span: saved.span,
        })
    }
}

pub(in crate::eval::machine) fn restore(
    saved: &SuspendedTaskHandlerSnapshot,
    owner_task: crate::driver::scheduler::TaskIdentity,
    revisions: &Arc<crate::api::code::PreparedRevisionSet>,
    contexts: &mut std::collections::HashMap<usize, crate::eval::machine::snapshot::RestoreContext>,
    table: &crate::eval::machine::snapshot::SuspensionRestoreTable,
    depth: &Rc<std::cell::Cell<u32>>,
    current: &crate::api::HostExecutionContext,
    options: super::super::suspension::checkpoint::RestoreOptions<'_>,
) -> Result<TaskHandlerContinuation, String> {
    use super::super::suspension::checkpoint::context;
    use crate::eval::machine::{frame::ProgramReturnFrame, scoped_call::ProgramCall};
    if saved.owner_task != owner_task {
        return Err("task Handler snapshot belongs to a different caller task".into());
    }
    let owner = revisions.execution_snapshot(&saved.owner)?;
    let owner_scope = saved.owner_scoped.then(|| ProgramScope {
        program: owner.clone(),
        handler_floor: saved.owner_handler_floor,
    });
    let mut scope = owner_scope.clone();
    let mut frames = Vec::with_capacity(saved.frames.len());
    for frame in saved.frames.iter().cloned() {
        let parent = scope
            .as_ref()
            .map_or(&owner, |scope| &scope.program)
            .clone();
        let ctx = context(
            contexts,
            &parent,
            revisions,
            table,
            options.locked_model,
            options.ordering,
            options.residual,
        );
        if let MachineFrameSnapshot::ProgramReturn(boundary) = frame {
            let call = ProgramCall::restore_target(
                ctx.restore_call_target(boundary.call)?,
                &parent.checked,
            )?;
            let destination = call.destination()?;
            frames.push(EvalFrame::ProgramReturn(Box::new(ProgramReturnFrame {
                site: Some(boundary.call_site.restore(ctx)?),
                parent: scope.clone(),
                parent_handler_floor: scope
                    .as_ref()
                    .map_or(saved.owner_handler_floor, |scope| scope.handler_floor),
                parent_program: parent,
                parent_model_policy: program::restore_model_policy(boundary.parent_model_policy),
                call,
                outer: boundary.outer.restore_with(ctx)?,
                span: boundary.span,
                retry_depth: boundary.retry_depth + saved.retry_base,
            })));
            scope = Some(ProgramScope {
                program: destination,
                handler_floor: boundary.handler_floor + saved.handler_base,
            });
        } else {
            frames.push(frame.restore_local(current, ctx)?);
        }
    }
    if scope.is_some() != saved.active_scoped {
        return Err("task Handler snapshot scope differs from its return boundaries".into());
    }
    let active = scope.as_ref().map_or(&owner, |scope| &scope.program);
    let ctx = context(
        contexts,
        active,
        revisions,
        table,
        options.locked_model,
        options.ordering,
        options.residual,
    );
    let wait = match &saved.wait {
        TaskHandlerWaitSnapshot::Ready(input) => {
            let mut machine = EvalMachine::new();
            machine.restore_input(Some((**input).clone()), ctx, Some(active))?;
            RootTaskWait::Ready(Box::new(
                machine
                    .take_task_input()
                    .expect("saved ready wait has an input"),
            ))
        }
        TaskHandlerWaitSnapshot::Await {
            target,
            continuation,
        } => RootTaskWait::Await {
            target: *target,
            continuation: continuation.clone().restore_with(ctx)?,
        },
        TaskHandlerWaitSnapshot::Run { scope } => RootTaskWait::Run { scope: *scope },
        TaskHandlerWaitSnapshot::Transfer {
            target,
            token,
            continuation,
            output,
            span,
        } => RootTaskWait::Transfer {
            target: *target,
            token: *token,
            continuation: continuation.clone().restore_with(ctx)?,
            output: *output,
            span: *span,
        },
    };
    let projection = ActionProjection::restore_checkpoint(&saved.projection, &owner, revisions)?;
    let count = u32::try_from(
        frames
            .iter()
            .filter(|frame| matches!(frame, EvalFrame::Call(_) | EvalFrame::ProgramReturn(_)))
            .count(),
    )
    .map_err(|_| "task Handler call-depth charge overflow")?;
    depth.set(
        depth
            .get()
            .checked_add(count)
            .ok_or("task Handler call-depth charge overflow")?,
    );
    Ok(TaskHandlerContinuation(Rc::new(RefCell::new(Some(
        SuspendedTaskHandler {
            child: saved.child,
            owner_task,
            handler: saved.handler,
            owner,
            owner_scope,
            owner_handler_floor: saved.owner_handler_floor,
            handler_base: saved.handler_base,
            retry_base: saved.retry_base,
            wait,
            scope,
            frames,
            handlers: saved.handlers.handlers.clone(),
            retries: saved.retries.attempts.clone(),
            model_policy: program::restore_model_policy(saved.model_policy.clone()),
            projection,
            calls: TaskCallLease {
                total: depth.clone(),
                count,
            },
            span: saved.span,
        },
    )))))
}

#[cfg(test)]
pub(crate) fn restore_standalone_for_test(
    saved: &SuspendedTaskHandlerSnapshot,
    machine: &EvalMachine,
    program: &CheckedExecutionProgram,
    owner_task: crate::driver::scheduler::TaskIdentity,
    current: &crate::api::HostExecutionContext,
) -> Result<TaskHandlerContinuation, String> {
    let revisions = Arc::new(crate::api::code::PreparedRevisionSet::new(
        program.checked.clone(),
    ));
    let mut contexts = std::collections::HashMap::from([(
        Arc::as_ptr(&program.checked) as usize,
        crate::eval::machine::snapshot::RestoreContext::for_task(
            program.clone(),
            current,
            None,
            Some(revisions.clone()),
            None,
            crate::eval::machine::snapshot::SharedFrameRestore::default(),
        ),
    )]);
    restore(
        saved,
        owner_task,
        &revisions,
        &mut contexts,
        &Rc::new(RefCell::new(Default::default())),
        &machine.active_call_depth,
        current,
        super::super::suspension::checkpoint::RestoreOptions {
            locked_model: &None,
            ordering: None,
            residual: &None,
        },
    )
}

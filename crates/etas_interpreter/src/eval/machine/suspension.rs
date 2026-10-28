use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use etas_core::{AnalysisDiagnosticCode, Span};

use super::{
    EvalMachine, MachinePoll,
    frame::EvalFrame,
    state::{MachineInput, ProgramScope},
};
use crate::{
    control::{
        Continuation, ContinuationInput, ControlSignal, ExecutionFault, PendingContinuation,
        PendingPerform,
    },
    eval::EvalContext,
    plan::code::ActionProjection,
    value::InterpValue,
};

pub(in crate::eval::machine) mod checkpoint;
mod drop;

/// A single-shot native execution segment, not a portable executable artifact.
#[derive(Clone, Debug)]
pub struct ProgramContinuation(Option<Rc<RefCell<Option<SuspendedProgram>>>>);

impl ProgramContinuation {
    pub(in crate::eval::machine) fn projected_action(
        &self,
    ) -> Result<crate::control::CheckedActionInvocation, String> {
        let state = self
            .0
            .as_ref()
            .ok_or("program continuation has no live identity")?
            .try_borrow()
            .map_err(|_| "program continuation is already being consumed")?;
        Ok(state
            .as_ref()
            .ok_or("program continuation has already been consumed")?
            .projection
            .target()
            .clone())
    }
    pub(in crate::eval::machine) fn checkpoint_identity(&self) -> Result<usize, String> {
        self.0
            .as_ref()
            .map(|inner| Rc::as_ptr(inner) as usize)
            .ok_or_else(|| "program continuation has no live identity".into())
    }
    fn new(program: SuspendedProgram) -> Self {
        Self(Some(Rc::new(RefCell::new(Some(program)))))
    }

    fn take_unique(&mut self) -> Option<SuspendedProgram> {
        Rc::try_unwrap(self.0.take()?).ok()?.into_inner()
    }
}

#[derive(Debug)]
struct SuspendedProgram {
    scope: ProgramScope,
    frames: Vec<EvalFrame>,
    continuation: Continuation,
    handlers: Vec<crate::orchestration::ActiveHandlerRecord>,
    retries: Vec<crate::orchestration::RetryAttemptRecord>,
    model_policy: crate::api::ModelExecutionPolicy,
    projection: ActionProjection,
    resuming_program: crate::value::CheckedExecutionProgram,
    depth: CallDepthLease,
    span: Span,
}

#[derive(Debug)]
struct CallDepthLease {
    total: Rc<Cell<u32>>,
    calls: u32,
}

impl Drop for CallDepthLease {
    fn drop(&mut self) {
        // Frames detached from the active stack remain charged until resumed or
        // discarded by finish. Only this private lease can release that charge.
        self.total.set(self.total.get() - self.calls);
    }
}

impl EvalMachine {
    pub(super) fn has_action_handler_in(
        &self,
        ctx: &EvalContext<'_>,
        pending: &PendingPerform,
        range: std::ops::Range<usize>,
    ) -> Result<bool, ExecutionFault> {
        let fail = |reason| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                pending.span,
                reason,
            )
        };
        let scopes = ctx.handler_stack.get(range).ok_or_else(|| {
            fail("action handler search has an invalid dynamic scope range".into())
        })?;
        if scopes.is_empty() {
            return Ok(false);
        }
        let frames = self
            .handler_scope_frames(&pending.continuation)
            .map_err(fail)?;
        for scope in scopes.iter().rev() {
            let frame = frames
                .get(&scope.id)
                .copied()
                .ok_or_else(|| fail("active handler scope has no owning lexical frame".into()))?;
            for handler in &scope.handlers {
                if ctx.handler_matches_perform(pending, handler, Some(frame))? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub(super) fn has_outer_action_handler(
        &self,
        ctx: &mut EvalContext<'_>,
        pending: &PendingPerform,
    ) -> Result<bool, ExecutionFault> {
        if ctx.handler_floor == 0 {
            return Ok(false);
        }
        let fail = |reason| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                pending.span,
                reason,
            )
        };
        let mut proof = pending
            .checked_action
            .clone()
            .ok_or_else(|| fail("resumable action has no checked invocation ABI".into()))?;
        let mut end = ctx.handler_floor;
        for frame in self.frames().iter().rev() {
            let EvalFrame::ProgramReturn(boundary) = frame else {
                continue;
            };
            let projection = ActionProjection::new(
                proof,
                boundary.call.action_projections().map_err(fail)?,
                &boundary.parent_program,
            )
            .map_err(fail)?;
            proof = projection.target().clone();
            let start = boundary
                .parent
                .as_ref()
                .map_or(0, |scope| scope.handler_floor);
            let relocated = relocated_perform(
                proof.clone(),
                boundary.span,
                Vec::new(),
                Continuation::BlockValue,
            )
            .map_err(fail)?;
            let parent = ctx.scoped_context(&boundary.parent_program, start);
            if self.has_action_handler_in(&parent, &relocated, start..end)? {
                return Ok(true);
            }
            end = start;
        }
        Ok(false)
    }

    fn handler_scope_frames<'a>(
        &'a self,
        pending: &'a Continuation,
    ) -> Result<
        std::collections::HashMap<crate::orchestration::HandlerScopeId, &'a crate::control::Frame>,
        String,
    > {
        let mut found = std::collections::HashMap::new();
        let mut work = vec![pending];
        for frame in self.frames() {
            match frame {
                EvalFrame::PureBoundary(_) => (),
                EvalFrame::Handler(frame) => {
                    if found.insert(frame.scope_id, &frame.frame).is_some() {
                        return Err("handler scope has multiple lexical frame owners".into());
                    }
                    work.push(&frame.inner);
                }
                EvalFrame::ProgramReturn(frame) => work.push(&frame.outer),
                EvalFrame::Block(frame) => work.push(&frame.continuation),
                EvalFrame::Call(frame) => work.push(&frame.continuation),
                EvalFrame::Expr(frame) => work.push(&frame.continuation),
                EvalFrame::Continuation(frame) => work.push(&frame.continuation),
                EvalFrame::ModelLoop(frame) => work.push(&frame.outer_continuation),
                EvalFrame::SourceToolReturn(frame) => {
                    work.push(&frame.model_loop.outer_continuation)
                }
                EvalFrame::Retry(_) => {}
            }
        }
        while let Some(continuation) = work.pop() {
            match continuation {
                Continuation::HandleBoundary {
                    scope_id,
                    frame,
                    inner,
                    ..
                } => {
                    if found.insert(*scope_id, frame).is_some() {
                        return Err("handler scope has multiple lexical frame owners".into());
                    }
                    work.push(inner);
                }
                Continuation::CallBoundary { outer, .. }
                | Continuation::HandlerDispatch { outer } => work.push(outer),
                Continuation::RestoreModelPolicy { inner, .. }
                | Continuation::ScopedModelPolicy { inner, .. } => work.push(inner),
                Continuation::Chain { inner, outer } => {
                    work.push(outer);
                    work.push(inner);
                }
                _ => {}
            }
        }
        Ok(found)
    }

    pub(super) fn suspend_program_action(
        &mut self,
        ctx: &mut EvalContext<'_>,
        mut pending: PendingPerform,
    ) -> MachinePoll {
        let span = pending.span;
        let fail = |reason| {
            MachinePoll::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                reason,
            ))
        };
        let Some(index) = self
            .stack
            .iter()
            .rposition(|frame| matches!(frame, EvalFrame::ProgramReturn(_)))
        else {
            return fail("cross-program action has no owning program boundary".into());
        };
        let Some(scope) = self.program_scope.clone() else {
            return fail("cross-program action has no native program scope".into());
        };
        let EvalFrame::ProgramReturn(boundary) = &self.stack[index] else {
            unreachable!()
        };
        if !Arc::ptr_eq(ctx.checked, &scope.program.checked)
            || ctx.handler_floor != scope.handler_floor
            || ctx.handler_stack.len() < scope.handler_floor
            || ctx.retry_stack.len() < boundary.retry_depth
        {
            return fail("action suspension disagrees with its checked dynamic scope".into());
        }
        let Some(proof) = pending.checked_action.clone() else {
            return fail("resumable action has no checked invocation ABI".into());
        };
        let projection = match boundary
            .call
            .action_projections()
            .and_then(|edges| ActionProjection::new(proof, edges, &boundary.parent_program))
        {
            Ok(projection) => projection,
            Err(reason) => return fail(reason),
        };
        let args = match projection.arguments(std::mem::take(&mut pending.args), &scope.program) {
            Ok(args) => args,
            Err(reason) => return fail(reason),
        };
        let retry_depth = boundary.retry_depth;
        let parent = boundary.parent.clone();
        let parent_policy = boundary.parent_model_policy.clone();
        let parent_span = boundary.span;
        let resuming_program = boundary.parent_program.clone();
        let mut frames = self.stack.split_off(index);
        let EvalFrame::ProgramReturn(boundary) = &mut frames[0] else {
            unreachable!()
        };
        let outer = std::mem::replace(&mut boundary.outer, Continuation::BlockValue);
        let calls = frames
            .iter()
            .filter(|frame| matches!(frame, EvalFrame::Call(_) | EvalFrame::ProgramReturn(_)))
            .count() as u32;
        let suspended = ProgramContinuation::new(SuspendedProgram {
            scope: scope.clone(),
            frames,
            continuation: std::mem::replace(&mut pending.continuation, Continuation::BlockValue),
            handlers: ctx.handler_stack.split_off(scope.handler_floor),
            retries: ctx.retry_stack.split_off(retry_depth),
            model_policy: std::mem::replace(&mut ctx.model_policy, parent_policy),
            projection: projection.clone(),
            resuming_program,
            depth: CallDepthLease {
                total: self.active_call_depth.clone(),
                calls,
            },
            span,
        });
        let relocated = match relocated_perform(
            projection.target().clone(),
            parent_span,
            args,
            Continuation::ProgramResume(suspended),
        ) {
            Ok(pending) => pending,
            Err(reason) => return fail(reason),
        };
        self.push_continuation_frames(outer);
        self.program_scope = parent;
        self.input = Some(MachineInput::Signal(ControlSignal::pending_perform(
            relocated,
        )));
        MachinePoll::ProgramChanged
    }

    pub(super) fn resume_program_continuation(
        &mut self,
        ctx: &mut EvalContext<'_>,
        continuation: ProgramContinuation,
        value: InterpValue,
    ) -> MachinePoll {
        let Some(state) = &continuation.0 else {
            return MachinePoll::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                crate::diagnostics::item_span(ctx.checked, ctx.entry_item),
                "program continuation has no owned execution segment",
            ));
        };
        let borrowed = state.try_borrow_mut();
        let Ok(mut slot) = borrowed else {
            return MachinePoll::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                crate::diagnostics::item_span(ctx.checked, ctx.entry_item),
                "program continuation is already being resumed",
            ));
        };
        let Some(mut saved) = slot.take() else {
            return MachinePoll::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                crate::diagnostics::item_span(ctx.checked, ctx.entry_item),
                "program continuation was already consumed",
            ));
        };
        let fail = |reason| {
            MachinePoll::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                saved.span,
                reason,
            ))
        };
        if !Rc::ptr_eq(&saved.depth.total, &self.active_call_depth) {
            return fail("program continuation belongs to a different invocation".into());
        }
        if !Arc::ptr_eq(ctx.checked, &saved.resuming_program.checked) {
            return fail(
                "program continuation resumed in a different parent checked program".into(),
            );
        }
        let resuming_program = ctx.checked_execution_program();
        let value = match saved.projection.resume(value, &resuming_program) {
            Ok(value) => value,
            Err(reason) => return fail(reason),
        };
        let Some(EvalFrame::ProgramReturn(boundary)) = saved.frames.first_mut() else {
            return fail("program continuation has no owning call boundary".into());
        };
        boundary.retry_depth = ctx.retry_stack.len();
        boundary.parent_model_policy = ctx.model_policy.clone();
        boundary.parent = self.program_scope.clone();
        boundary.parent_program = resuming_program;
        saved.scope.handler_floor = ctx.handler_stack.len();
        ctx.handler_stack.append(&mut saved.handlers);
        ctx.retry_stack.append(&mut saved.retries);
        ctx.model_policy = saved.model_policy;
        self.stack.append(&mut saved.frames);
        saved.depth.calls = 0;
        self.program_scope = Some(saved.scope);
        self.input = Some(MachineInput::Signal(ControlSignal::pending_continuation(
            PendingContinuation {
                continuation: saved.continuation,
                input: ContinuationInput::Value(value),
            },
        )));
        MachinePoll::ProgramChanged
    }
}

fn relocated_perform(
    proof: crate::control::CheckedActionInvocation,
    span: Span,
    args: Vec<InterpValue>,
    continuation: Continuation,
) -> Result<PendingPerform, String> {
    let action = match &proof.effect {
        etas_effects::Effect::Action(action) => action,
        etas_effects::Effect::AppliedAction(instance) => &instance.action,
        _ => return Err("relocated action has no canonical action identity".into()),
    };
    let registry = &proof.program.effect_registry;
    let owner = registry
        .tag_name(action.tag)
        .ok_or("relocated action has no checked owner")?;
    let name = registry
        .action_name(action.tag, action.action)
        .ok_or("relocated action has no checked declaration")?;
    let segments = owner.split('.').collect::<Vec<_>>();
    let action = etas_hir::ResolvedActionRef {
        action: name.into(),
        action_symbol: etas_hir::ResolveResult::Unresolved,
        span,
        effect: etas_hir::HirEffectRef {
            path: etas_hir::unresolved_path_from_segments(&segments, span),
            args: Vec::new(),
            span,
        },
    };
    Ok(PendingPerform {
        checked_action: Some(proof),
        default_boundary: None,
        forwarded_call: None,
        expr: None,
        action,
        error_type: None,
        inference: None,
        args,
        span,
        continuation,
    })
}

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

#[derive(Clone, Debug)]
pub(in crate::eval::machine) enum SuspendedExecution {
    Program(ProgramContinuation),
    Task(super::TaskHandlerContinuation),
}

impl SuspendedExecution {
    pub(in crate::eval::machine) fn identity(&self) -> Result<usize, String> {
        match self {
            Self::Program(value) => value.checkpoint_identity(),
            Self::Task(value) => value.checkpoint_identity(),
        }
    }
    pub(in crate::eval::machine) fn projected_action(
        &self,
    ) -> Result<crate::control::CheckedActionInvocation, String> {
        match self {
            Self::Program(value) => value.projected_action(),
            Self::Task(value) => value.projected_action(),
        }
    }
    pub(in crate::eval::machine) fn capture(
        &self,
        context: &mut super::snapshot::CaptureContext,
    ) -> Result<crate::orchestration::SuspendedExecutionSnapshot, String> {
        use crate::orchestration::SuspendedExecutionSnapshot as Snapshot;
        match self {
            Self::Program(value) => value.capture(context).map(Snapshot::Program),
            Self::Task(value) => value.capture(context).map(Snapshot::Task),
        }
    }
}

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
    agent_context: Option<Box<crate::control::AgentRequestContext>>,
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

/// An await transfer owns its eventual result projections while its lexical
/// frames are installed on the parked caller and accessible to child actions.
#[derive(Debug)]
pub(crate) struct ParkedContinuation {
    pub(crate) continuation: Continuation,
    pub(crate) projections: Option<Vec<(crate::value::CheckedExecutionProgram, ActionProjection)>>,
}

impl EvalMachine {
    pub(super) fn has_action_handler_in(
        &self,
        ctx: &EvalContext<'_>,
        pending: &PendingPerform,
        range: std::ops::Range<usize>,
        memory_target: Option<&crate::control::handler_activation::SourceActionTarget>,
    ) -> Result<bool, ExecutionFault> {
        let fail = |reason| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                pending.span,
                reason,
            )
        };
        if range.end <= ctx.task.inherited_handlers {
            return Ok(false);
        }
        let range = range.start.max(ctx.task.inherited_handlers)..range.end;
        let scopes = ctx.task.handler_stack.get(range).ok_or_else(|| {
            fail("action handler search has an invalid dynamic scope range".into())
        })?;
        if scopes.is_empty() {
            return Ok(false);
        }
        let frames = self
            .handler_scope_frames(&pending.continuation)
            .map_err(fail)?;
        let action = ctx.checked_perform_action(pending)?;
        for scope in scopes.iter().rev() {
            for handler in &scope.handlers {
                // A task can retain an enclosing handler's dynamic record while
                // its lexical continuation belongs to the parked parent. An
                // unrelated action needs no frame or selector instantiation.
                if let Some(actual) = action.as_ref().and_then(action_identity)
                    && action_identity(ctx.checked_handler_action(handler)?) != Some(actual)
                {
                    continue;
                }
                let frame = frames.get(&scope.id).copied().ok_or_else(|| {
                    fail("active handler scope has no owning lexical frame".into())
                })?;
                if ctx.handler_matches_perform(pending, handler, Some(frame), memory_target)? {
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
        let proof = pending
            .checked_action
            .as_ref()
            .ok_or_else(|| fail("resumable action has no checked invocation ABI".into()))?;
        let mut identity = action_identity(&proof.effect)
            .ok_or_else(|| fail("resumable action has no checked action identity".into()))?
            .clone();
        let memory_target = ctx.source_handler_target(pending);
        let mut edges = Vec::new();
        let mut end = ctx.handler_floor;
        for frame in self.frames().iter().rev() {
            let EvalFrame::ProgramReturn(boundary) = frame else {
                continue;
            };
            for edge in boundary.call.action_projections().map_err(fail)? {
                let Some(action) = edge.corresponding_action(&identity) else {
                    return Ok(false);
                };
                identity = action;
                edges.push(edge);
            }
            let start = boundary.parent_handler_floor;
            let parent = ctx.scoped_context(&boundary.parent_program, start);
            let range = start.max(parent.task.inherited_handlers)..end;
            if !range.is_empty() {
                let scopes = parent.task.handler_stack.get(range).ok_or_else(|| {
                    fail("action handler search has an invalid dynamic scope range".into())
                })?;
                // Relocate the payload ABI only if an arm can handle this action.
                if has_action_family(
                    &parent,
                    scopes.iter().rev().flat_map(|scope| &scope.handlers),
                    &identity,
                )? {
                    let projection = ActionProjection::new(
                        proof.clone(),
                        edges.clone(),
                        &boundary.parent_program,
                    )
                    .map_err(fail)?;
                    let relocated = relocated_perform(
                        projection.target().clone(),
                        boundary.span,
                        Vec::new(),
                        Continuation::BlockValue,
                    )
                    .map_err(fail)?;
                    if self.has_action_handler_in(
                        &parent,
                        &relocated,
                        start..end,
                        memory_target.as_ref(),
                    )? {
                        return Ok(true);
                    }
                }
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
                EvalFrame::PureBoundary(_) | EvalFrame::ResidualScope(_) => (),
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
                EvalFrame::ModelRequest(frame) => work.push(&frame.outer_continuation),
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
                | Continuation::HandlerDispatch { outer, .. }
                | Continuation::ReenterHandler { boundary: outer } => work.push(outer),
                Continuation::RestoreModelPolicy { inner, .. }
                | Continuation::AgentExecutionScope { inner, .. }
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
            || ctx.task.handler_stack.len() < scope.handler_floor
            || ctx.task.retry_stack.len() < boundary.retry_depth
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
        let parent_agent_context = boundary.parent_agent_context.clone();
        let parent_span = boundary.span;
        let resuming_program = boundary.parent_program.clone();
        let mut frames = self.stack.split_off(index);
        self.resync_residual_mirror();
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
            handlers: ctx.task.handler_stack.split_off(scope.handler_floor),
            retries: ctx.task.retry_stack.split_off(retry_depth),
            model_policy: std::mem::replace(&mut ctx.task.model_policy, parent_policy),
            agent_context: std::mem::replace(&mut ctx.task.agent_context, parent_agent_context),
            projection: projection.clone(),
            resuming_program,
            depth: CallDepthLease {
                total: self.active_call_depth.clone(),
                calls,
            },
            span,
        });
        let mut relocated = match relocated_perform(
            projection.target().clone(),
            parent_span,
            args,
            Continuation::ProgramResume(suspended),
        ) {
            Ok(pending) => pending,
            Err(reason) => return fail(reason),
        };
        // The relocated request is the same semantic attempt: it keeps the
        // permit committed by every scope that observed it.
        relocated.residual = pending.residual.take();
        relocated.inference = pending.inference.take();
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
        let responder = ctx.checked_execution_program();
        let (next, projection) = match self.attach_program_continuation(
            ctx.task,
            &responder,
            continuation,
            crate::diagnostics::item_span(ctx.checked, ctx.entry_item),
        ) {
            Ok(saved) => saved,
            Err(fault) => return MachinePoll::Fault(fault),
        };
        match projection.resume(value, &responder) {
            Ok(value) => {
                self.resume(ControlSignal::pending_continuation(PendingContinuation {
                    continuation: next,
                    input: ContinuationInput::Value(value),
                }));
                MachinePoll::ProgramChanged
            }
            Err(message) => MachinePoll::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                crate::diagnostics::item_span(ctx.checked, ctx.entry_item),
                message,
            )),
        }
    }

    fn attach_program_continuation(
        &mut self,
        task: &mut crate::eval::TaskLocalState,
        responder: &crate::value::CheckedExecutionProgram,
        continuation: ProgramContinuation,
        span: Span,
    ) -> Result<(Continuation, ActionProjection), ExecutionFault> {
        let fail = |message| {
            ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, message)
        };
        let state = continuation
            .0
            .as_ref()
            .ok_or_else(|| fail("program continuation has no owned execution segment"))?;
        let mut slot = state
            .try_borrow_mut()
            .map_err(|_| fail("program continuation is already being resumed"))?;
        let mut saved = slot
            .take()
            .ok_or_else(|| fail("program continuation was already consumed"))?;
        if !Rc::ptr_eq(&saved.depth.total, &self.active_call_depth) {
            return Err(fail(
                "program continuation belongs to a different invocation",
            ));
        }
        if !Arc::ptr_eq(&responder.checked, &saved.resuming_program.checked) {
            return Err(fail(
                "program continuation resumed in a different parent checked program",
            ));
        }
        let Some(EvalFrame::ProgramReturn(boundary)) = saved.frames.first_mut() else {
            return Err(fail("program continuation has no owning call boundary"));
        };
        boundary.retry_depth = task.retry_stack.len();
        boundary.parent_model_policy = task.model_policy.clone();
        boundary.parent_agent_context = task.agent_context.clone();
        boundary.parent = self.program_scope.clone();
        boundary.parent_program = responder.clone();
        saved.scope.handler_floor = task.handler_stack.len();
        task.handler_stack.append(&mut saved.handlers);
        task.retry_stack.append(&mut saved.retries);
        task.model_policy = saved.model_policy;
        task.agent_context = saved.agent_context;
        self.stack.append(&mut saved.frames);
        self.resync_residual_mirror();
        saved.depth.calls = 0;
        self.program_scope = Some(saved.scope);
        Ok((saved.continuation, saved.projection))
    }

    pub(crate) fn park_transfer(
        &mut self,
        ctx: &mut EvalContext<'_>,
        token: crate::value::ContinuationTokenValue,
        mut continuation: Continuation,
        span: Span,
    ) -> Result<ParkedContinuation, ExecutionFault> {
        self.begin_transfer(ctx.task, token);
        let mut responder = self
            .program_scope
            .as_ref()
            .map(|scope| scope.program.clone())
            .or_else(|| {
                self.checkpoint_root
                    .as_ref()
                    .map(|root| root.program.clone())
            })
            .unwrap_or_else(|| ctx.checked_execution_program());
        let mut projections = Vec::new();
        loop {
            match continuation {
                Continuation::Chain { inner, outer } => {
                    if let Continuation::RestoreHandlerArms { scope_id, handlers } = &*inner {
                        ctx.task
                            .restore_handler_arms(*scope_id, handlers.clone())
                            .map_err(|reason| {
                                ExecutionFault::new(
                                    AnalysisDiagnosticCode::MissingCheckedFact,
                                    span,
                                    reason,
                                )
                            })?;
                        continuation = outer.into_value();
                        continue;
                    }
                    self.push_continuation_frames(outer.into_value());
                    continuation = inner.into_value();
                }
                Continuation::ProgramResume(saved) => {
                    let (next, projection) =
                        self.attach_program_continuation(ctx.task, &responder, saved, span)?;
                    projections.push((responder, projection));
                    responder = self
                        .program_scope
                        .as_ref()
                        .expect("attached program scope")
                        .program
                        .clone();
                    continuation = next;
                }
                Continuation::ReenterHandler { boundary } => {
                    ctx.enter_saved_handler(&boundary)?;
                    continuation = boundary.into_value();
                }
                Continuation::HandleBoundary {
                    scope_id,
                    handlers,
                    span,
                    frame,
                    inner,
                } => {
                    self.push_continuation_frames(Continuation::HandleBoundary {
                        scope_id,
                        handlers,
                        span,
                        frame,
                        inner: Continuation::BlockValue.into(),
                    });
                    continuation = inner.into_value();
                }
                Continuation::AgentExecutionScope { state, inner } => {
                    continuation = ctx.agent_scope_continuation(*state, inner.into_value());
                }
                Continuation::ScopedModelPolicy { policy, inner } => {
                    let previous = std::mem::replace(&mut ctx.task.model_policy, *policy);
                    self.push_continuation_frames(Continuation::RestoreModelPolicy {
                        previous: Box::new(previous),
                        inner: Continuation::BlockValue.into(),
                    });
                    continuation = inner.into_value();
                }
                Continuation::RestoreModelPolicy { previous, inner } => {
                    ctx.task.model_policy = *previous;
                    continuation = inner.into_value();
                }
                Continuation::CallBoundary { outer, .. }
                | Continuation::HandlerDispatch {
                    outer,
                    deferred: None,
                } => {
                    continuation = outer.into_value();
                }
                _ => {
                    return Ok(ParkedContinuation {
                        continuation,
                        projections: Some(projections),
                    });
                }
            }
        }
    }
}

pub(super) fn collect_park_handlers(
    owner: &crate::value::CheckedExecutionProgram,
    continuation: &Continuation,
    sites: &mut Vec<(
        crate::value::CheckedExecutionProgram,
        super::frame::HandlerFrame,
        bool,
    )>,
    edges: &mut Vec<crate::plan::code::RevisionProjection>,
) -> Result<(), String> {
    let mut work = vec![(owner.clone(), continuation.clone())];
    while let Some((owner, continuation)) = work.pop() {
        match continuation {
            Continuation::ProgramResume(program) => {
                let state = program
                    .0
                    .as_ref()
                    .ok_or("program continuation has no owned segment")?
                    .try_borrow()
                    .map_err(|_| "program continuation is already being resumed")?;
                let saved = state
                    .as_ref()
                    .ok_or("program continuation has already been consumed")?;
                let owner = &saved.scope.program;
                for frame in &saved.frames {
                    match frame {
                        EvalFrame::ProgramReturn(boundary) => {
                            edges.extend(boundary.call.action_projections()?)
                        }
                        EvalFrame::Handler(handler) => {
                            sites.push((owner.clone(), handler.clone(), true))
                        }
                        _ => {}
                    }
                }
                work.push((owner.clone(), saved.continuation.clone()));
            }
            Continuation::HandleBoundary {
                scope_id,
                handlers,
                span,
                frame,
                inner,
            } => {
                sites.push((
                    owner.clone(),
                    super::frame::HandlerFrame {
                        scope_id,
                        handlers,
                        span,
                        frame,
                        inner: Continuation::BlockValue.into(),
                    },
                    true,
                ));
                work.push((owner, inner.into_value()));
            }
            Continuation::Chain { inner, outer } => {
                work.push((owner.clone(), outer.into_value()));
                work.push((owner, inner.into_value()));
            }
            Continuation::ReenterHandler { boundary: inner }
            | Continuation::HandlerDispatch { outer: inner, .. }
            | Continuation::CallBoundary { outer: inner, .. }
            | Continuation::RestoreModelPolicy { inner, .. }
            | Continuation::ScopedModelPolicy { inner, .. }
            | Continuation::AgentExecutionScope { inner, .. } => {
                work.push((owner, inner.into_value()))
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn has_action_family<'a>(
    ctx: &EvalContext<'_>,
    handlers: impl IntoIterator<Item = &'a crate::orchestration::ActiveHandlerArmRecord>,
    identity: &etas_effects::ActionRef,
) -> Result<bool, ExecutionFault> {
    for handler in handlers {
        if action_identity(ctx.checked_handler_action(handler)?) == Some(identity) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn action_identity(effect: &etas_effects::Effect) -> Option<&etas_effects::ActionRef> {
    match effect {
        etas_effects::Effect::Action(action) => Some(action),
        etas_effects::Effect::AppliedAction(instance) => Some(&instance.action),
        _ => None,
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
        residual: None,
    })
}

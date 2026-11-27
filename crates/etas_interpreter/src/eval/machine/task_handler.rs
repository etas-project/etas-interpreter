use std::{cell::RefCell, rc::Rc, sync::Arc};

use etas_core::{AnalysisDiagnosticCode, Span};

use super::{
    EvalMachine, MachinePoll,
    frame::EvalFrame,
    state::{MachineInput, ProgramScope},
    suspension::{action_identity, has_action_family},
};
use crate::{
    control::{Continuation, ControlSignal, ExecutionFault, PendingPerform, PendingTaskControl},
    driver::scheduler::{RootTaskWait, TaskHandle},
    eval::EvalContext,
    orchestration::{ActiveHandlerRecord, HandlerScopeId, RetryAttemptRecord},
    plan::code::ActionProjection,
    value::{
        CheckedExecutionProgram, InterpValue,
        graph::{self, Node},
    },
};

pub(crate) mod checkpoint;

/// The scheduler owns the child machine; this single-shot segment owns only
/// the caller's suspended wait and the frames above its lexical handler.
#[derive(Clone, Debug)]
pub struct TaskHandlerContinuation(Rc<RefCell<Option<SuspendedTaskHandler>>>);

#[derive(Clone, Debug)]
pub(crate) struct WeakTaskHandlerContinuation(std::rc::Weak<RefCell<Option<SuspendedTaskHandler>>>);

impl TaskHandlerContinuation {
    pub(crate) fn downgrade(&self) -> WeakTaskHandlerContinuation {
        WeakTaskHandlerContinuation(Rc::downgrade(&self.0))
    }
}

impl WeakTaskHandlerContinuation {
    pub(crate) fn upgrade(&self) -> Option<TaskHandlerContinuation> {
        self.0.upgrade().map(TaskHandlerContinuation)
    }
}

#[derive(Debug)]
struct SuspendedTaskHandler {
    child: TaskHandle,
    owner_task: crate::driver::scheduler::TaskIdentity,
    handler: HandlerScopeId,
    owner: CheckedExecutionProgram,
    owner_scope: Option<ProgramScope>,
    owner_handler_floor: usize,
    handler_base: usize,
    retry_base: usize,
    wait: RootTaskWait,
    scope: Option<ProgramScope>,
    frames: Vec<EvalFrame>,
    handlers: Vec<ActiveHandlerRecord>,
    retries: Vec<RetryAttemptRecord>,
    model_policy: crate::api::ModelExecutionPolicy,
    host_context: Option<crate::eval::request_context::AgentHostContextLease>,
    record_context: Option<crate::control::RecordContextLease>,
    borrowed_handler: Option<super::frame::HandlerFrame>,
    projection: ActionProjection,
    calls: TaskCallLease,
    span: Span,
}

#[derive(Debug)]
struct TaskCallLease {
    total: Rc<std::cell::Cell<u32>>,
    count: u32,
}
impl Drop for TaskCallLease {
    fn drop(&mut self) {
        self.total.set(self.total.get() - self.count);
    }
}

impl EvalMachine {
    /// Stop an arm whose originating request was cancelled, preserving the
    /// lexical owner's suspended wait and all unrelated requests.
    pub(crate) fn abandon_cancelled_agent_handler(
        &mut self,
        ctx: &mut EvalContext<'_>,
    ) -> Result<bool, ExecutionFault> {
        let span = crate::diagnostics::item_span(ctx.checked, ctx.entry_item);
        let fail = |reason: String| {
            ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, reason)
        };
        let Some(continuation) = ctx
            .task
            .cancelled_agent_handler()
            .map_err(|error| fail(error.to_string()))?
        else {
            return Ok(false);
        };
        let Some(mut saved) = continuation.0.borrow_mut().take() else {
            return Err(fail(
                "cancelled Agent Handler lost its suspended owner".into(),
            ));
        };
        if saved.owner_task != ctx.task.identity
            || !Rc::ptr_eq(&saved.calls.total, &self.active_call_depth)
        {
            return Err(fail(
                "cancelled Agent Handler belongs to another machine".into(),
            ));
        }
        let at = self.stack.iter().position(|frame| {
            matches!(frame, EvalFrame::Handler(handler) if handler.scope_id == saved.handler)
        }).ok_or_else(|| fail("cancelled Agent Handler lost its lexical boundary".into()))?;
        self.input = None;
        self.root_wait = None;
        while self.stack.len() > at + 1 {
            match self.pop_frame().expect("Handler suffix exists") {
                EvalFrame::ResidualScope(scope) => {
                    ctx.exit_residual_scope(
                        &scope,
                        crate::control::residual::ResidualExitKind::Cancellation,
                        saved.span,
                    )?;
                }
                EvalFrame::ProgramReturn(boundary) => {
                    ctx.task.agent_context = boundary.parent_agent_context;
                }
                _ => {}
            }
        }
        while ctx.task.handler_stack.len() > saved.handler_base {
            let scope = ctx
                .task
                .handler_stack
                .last()
                .expect("nested Handler exists")
                .id;
            ctx.scoped_context(&saved.owner, saved.owner_handler_floor)
                .close_handler_scope(scope, saved.span)?;
        }
        ctx.task.retry_stack.truncate(saved.retry_base);
        let handler = saved.borrowed_handler.take().ok_or_else(|| {
            fail("cancelled Agent Handler lost its original owner boundary".into())
        })?;
        ctx.task
            .restore_handler_arms(handler.scope_id, handler.handlers.clone())
            .map_err(fail)?;
        self.stack[at] = EvalFrame::Handler(handler);
        drop(saved.host_context.take());
        drop(saved.record_context.take());
        ctx.task.handler_stack.append(&mut saved.handlers);
        ctx.task.retry_stack.append(&mut saved.retries);
        ctx.task.model_policy = saved.model_policy;
        self.stack.append(&mut saved.frames);
        self.resync_residual_mirror();
        saved.calls.count = 0;
        self.program_scope = saved.scope;
        self.root_wait = Some(saved.wait);
        self.resume(ControlSignal::Task(Box::new(
            PendingTaskControl::RestoreHandlerWait { span: saved.span },
        )));
        Ok(true)
    }

    pub(crate) fn take_task_input(&mut self) -> Option<MachineInput> {
        self.input.take()
    }
    pub(crate) fn restore_task_input(&mut self, input: MachineInput) {
        self.input = Some(input);
    }

    pub(crate) fn relay_task_handler(
        &mut self,
        ctx: &mut EvalContext<'_>,
        child: TaskHandle,
        child_logical_identity: crate::driver::scheduler::TaskIdentity,
        child_host_context: Option<crate::eval::request_context::AgentHostContext>,
        child_record_context: Option<crate::control::RecordContextInput>,
        child_machine: &EvalMachine,
        child_program: &CheckedExecutionProgram,
        pending: &PendingPerform,
        inherited: &[ActiveHandlerRecord],
    ) -> Result<Option<HandlerScopeId>, ExecutionFault> {
        let fail = |reason| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                pending.span,
                reason,
            )
        };
        let proof = pending
            .checked_action
            .clone()
            .ok_or_else(|| fail("task action has no checked invocation".into()))?;
        let scopes = super::snapshot::program::capture_scopes(self).map_err(fail)?;
        let root = self
            .checkpoint_root
            .as_ref()
            .map(|root| root.program.clone())
            .unwrap_or_else(|| ctx.checked_execution_program());
        let mut sites = Vec::new();
        let mut scope = None;
        for (index, frame) in self.stack.iter().enumerate() {
            if let EvalFrame::ProgramReturn(_) = frame {
                scope = Some(scopes[&index].clone());
            }
            if let EvalFrame::Handler(handler) = frame {
                if inherited.iter().any(|record| record.id == handler.scope_id) {
                    sites.push((index, scope.clone(), active_handler(ctx, handler)));
                }
            }
        }
        let mut edges = Vec::new();
        for machine in [child_machine, &*self] {
            for frame in machine.stack.iter().rev() {
                if let EvalFrame::ProgramReturn(boundary) = frame {
                    edges.extend(boundary.call.action_projections().map_err(fail)?);
                }
            }
        }
        for (index, scope, handler) in sites.into_iter().rev() {
            let owner = scope.as_ref().map_or(&root, |scope| &scope.program);
            let root_floor = self
                .stack
                .iter()
                .find_map(|frame| match frame {
                    EvalFrame::ProgramReturn(boundary) => Some(boundary.parent_handler_floor),
                    _ => None,
                })
                .unwrap_or(ctx.handler_floor);
            let floor = scope
                .as_ref()
                .map_or(root_floor, |scope| scope.handler_floor);
            let Some((projection, mut action)) =
                match_task_action(ctx, pending, &proof, owner, floor, &handler, &edges)?
            else {
                continue;
            };
            if child_logical_identity != ctx.task.logical_identity
                && pending
                    .args
                    .iter()
                    .any(|value| graph::has_scoped_bindings(Node::Value(value)))
            {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    pending.span,
                    "task action payload captures a local var; capture an immutable snapshot instead",
                ));
            }
            action.args = projection
                .arguments(pending.args.clone(), child_program)
                .map_err(fail)?;
            let at = ctx
                .task
                .handler_stack
                .iter()
                .position(|record| record.id == handler.scope_id)
                .expect("lexical handler has a dynamic record");
            let retry_count = self.stack[index + 1..]
                .iter()
                .filter(|frame| matches!(frame, EvalFrame::Retry(_)))
                .count();
            let retry_at = ctx.task.retry_stack.len() - retry_count;
            let owner_policy = self.stack[index + 1..]
                .iter()
                .find_map(|frame| match frame {
                    EvalFrame::ProgramReturn(boundary) => {
                        Some(boundary.parent_model_policy.clone())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| ctx.task.model_policy.clone());
            let frames = self.stack.split_off(index + 1);
            let count = frames
                .iter()
                .filter(|frame| matches!(frame, EvalFrame::Call(_) | EvalFrame::ProgramReturn(_)))
                .count() as u32;
            let saved = SuspendedTaskHandler {
                child,
                owner_task: ctx.task.identity,
                handler: handler.scope_id,
                owner: owner.clone(),
                owner_scope: scope.clone(),
                owner_handler_floor: floor,
                handler_base: at + 1,
                retry_base: retry_at,
                wait: self
                    .root_wait
                    .take()
                    .expect("caller owns its scheduler wait"),
                scope: self.program_scope.take(),
                frames,
                handlers: ctx.task.handler_stack.split_off(at + 1),
                retries: ctx.task.retry_stack.split_off(retry_at),
                model_policy: std::mem::replace(&mut ctx.task.model_policy, owner_policy),
                host_context: child_host_context.clone().map(|context| {
                    ctx.task
                        .borrow_agent_host_context(context, Some(handler.scope_id))
                }),
                record_context: child_record_context
                    .clone()
                    .map(|(source, guards)| ctx.task.borrow_record_context(source, guards)),
                borrowed_handler: child_host_context.as_ref().map(|_| handler.clone()),
                projection,
                calls: TaskCallLease {
                    total: self.active_call_depth.clone(),
                    count,
                },
                span: pending.span,
            };
            let continuation = TaskHandlerContinuation(Rc::new(RefCell::new(Some(saved))));
            if let Some(loan) = continuation
                .0
                .borrow()
                .as_ref()
                .and_then(|saved| saved.host_context.as_ref())
            {
                loan.attach(&continuation);
            }
            action.continuation = Continuation::TaskResume(continuation);
            self.program_scope = scope;
            self.resync_residual_mirror();
            self.input = Some(MachineInput::Signal(ControlSignal::pending_perform(action)));
            return Ok(Some(handler.scope_id));
        }
        Ok(None)
    }

    pub(crate) fn dormant_task_handler(
        &self,
        ctx: &mut EvalContext<'_>,
        child_machine: &EvalMachine,
        _child_program: &CheckedExecutionProgram,
        pending: &PendingPerform,
        inherited: &[ActiveHandlerRecord],
        continuations: &[(CheckedExecutionProgram, Continuation)],
    ) -> Result<Option<HandlerScopeId>, ExecutionFault> {
        let fail = |message| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                pending.span,
                message,
            )
        };
        let proof = pending
            .checked_action
            .as_ref()
            .ok_or_else(|| fail("task action has no checked invocation".to_owned()))?;
        let scopes = super::snapshot::program::capture_scopes(self).map_err(fail)?;
        let root = self
            .checkpoint_root
            .as_ref()
            .map(|root| root.program.clone())
            .unwrap_or_else(|| ctx.checked_execution_program());
        let mut sites = Vec::new();
        let mut edges = Vec::new();
        let mut scope = None;
        for (index, frame) in self.stack.iter().enumerate() {
            match frame {
                EvalFrame::ProgramReturn(_) => scope = Some(&scopes[&index]),
                EvalFrame::Handler(handler) => sites.push((
                    scope.map_or(&root, |s| &s.program).clone(),
                    active_handler(ctx, handler),
                    false,
                )),
                _ => {}
            }
        }
        for machine in [child_machine, self] {
            for frame in machine.stack.iter().rev() {
                if let EvalFrame::ProgramReturn(boundary) = frame {
                    edges.extend(boundary.call.action_projections().map_err(fail)?);
                }
            }
        }
        for (owner, continuation) in continuations {
            super::suspension::collect_park_handlers(owner, continuation, &mut sites, &mut edges)
                .map_err(fail)?;
        }
        for record in inherited.iter().rev() {
            if let Some((owner, handler, dormant)) =
                sites.iter().find(|(_, h, _)| h.scope_id == record.id)
                && match_task_action(ctx, pending, proof, owner, 0, handler, &edges)?.is_some()
            {
                return Ok(dormant.then_some(record.id));
            }
        }
        Ok(None)
    }

    pub(super) fn resume_task_handler(
        &mut self,
        ctx: &mut EvalContext<'_>,
        continuation: TaskHandlerContinuation,
        value: InterpValue,
    ) -> MachinePoll {
        let Some(mut saved) = continuation.0.borrow_mut().take() else {
            return MachinePoll::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                crate::diagnostics::item_span(ctx.checked, ctx.entry_item),
                "task action continuation was already consumed",
            ));
        };
        let fail = |reason| {
            MachinePoll::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                saved.span,
                reason,
            ))
        };
        if saved.owner_task != ctx.task.identity
            || !Rc::ptr_eq(&saved.calls.total, &self.active_call_depth)
        {
            return fail("task handler belongs to another caller machine".into());
        }
        let owner_scope = self.program_scope.clone();
        let owner = owner_scope.as_ref().map_or_else(
            || ctx.checked_execution_program(),
            |scope| scope.program.clone(),
        );
        if !Arc::ptr_eq(&owner.checked, &saved.owner.checked)
            || owner_scope.is_some() != saved.owner_scope.is_some()
            || owner_scope
                .as_ref()
                .map_or(ctx.handler_floor, |scope| scope.handler_floor)
                != saved.owner_handler_floor
        {
            return fail("task handler resumed outside its owning program scope".into());
        }
        // Prepared code can select another Arc for the same checked callable.
        // Reattach the suffix to the actual caller instance still below the
        // Handler; inner return boundaries retain their own destination plans.
        if let Some(boundary) = saved.frames.iter_mut().find_map(|frame| match frame {
            EvalFrame::ProgramReturn(boundary) => Some(boundary),
            _ => None,
        }) {
            boundary.parent = owner_scope;
            boundary.parent_program = owner;
        } else {
            saved.scope = owner_scope;
        }
        if graph::has_scoped_bindings(Node::Value(&value)) {
            let child_logical_identity = match ctx.scheduler.task_logical_identity(
                crate::driver::scheduler::TaskIdentity::Scheduled(saved.child.task),
            ) {
                Ok(identity) => identity,
                Err(reason) => return fail(reason.to_string()),
            };
            if child_logical_identity != ctx.task.logical_identity {
                return MachinePoll::Fault(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    saved.span,
                    "task action response captures a local var; capture an immutable snapshot instead",
                ));
            }
        }
        let value = match saved
            .projection
            .resume(value, &ctx.checked_execution_program())
        {
            Ok(value) => value,
            Err(reason) => return fail(reason),
        };
        drop(saved.host_context.take());
        drop(saved.record_context.take());
        ctx.task.handler_stack.append(&mut saved.handlers);
        ctx.task.retry_stack.append(&mut saved.retries);
        ctx.task.model_policy = saved.model_policy;
        self.stack.append(&mut saved.frames);
        self.resync_residual_mirror();
        saved.calls.count = 0;
        self.program_scope = saved.scope;
        self.root_wait = Some(saved.wait);
        MachinePoll::Yield(super::PendingBoundary::Task(Box::new(
            PendingTaskControl::ResumeHandled {
                task: saved.child,
                handler: saved.handler,
                value,
                span: saved.span,
            },
        )))
    }
}

// A frame may have been captured while its arm was masked. The task record
// owns the currently enabled arms, including those restored by resume.
fn active_handler(
    ctx: &EvalContext<'_>,
    frame: &super::frame::HandlerFrame,
) -> super::frame::HandlerFrame {
    let record = ctx
        .task
        .handler_stack
        .iter()
        .rev()
        .find(|record| record.id == frame.scope_id)
        .expect("active handler frame has a dynamic record");
    super::frame::HandlerFrame {
        handlers: record.handlers.clone(),
        ..frame.clone()
    }
}

fn match_task_action(
    ctx: &mut EvalContext<'_>,
    pending: &PendingPerform,
    proof: &crate::control::CheckedActionInvocation,
    owner: &CheckedExecutionProgram,
    floor: usize,
    handler: &super::frame::HandlerFrame,
    edges: &[crate::plan::code::RevisionProjection],
) -> Result<Option<(ActionProjection, PendingPerform)>, ExecutionFault> {
    let fail = |reason| {
        ExecutionFault::new(
            AnalysisDiagnosticCode::MissingCheckedFact,
            pending.span,
            reason,
        )
    };
    let identity = action_identity(&proof.effect)
        .ok_or_else(|| fail("task action has no checked action identity".into()))?;
    // Each edge is an actual checked call on the child's or caller's captured stack.
    let mut routes = vec![(proof.program.clone(), identity.clone(), Vec::new())];
    let mut seen = std::collections::HashSet::new();
    while let Some((cursor, identity, path)) = routes.pop() {
        if Arc::ptr_eq(&cursor, &owner.checked) {
            let memory_target = ctx.source_memory_handler_target(pending);
            let owner_ctx = ctx.scoped_context(owner, floor);
            if !has_action_family(&owner_ctx, &handler.handlers, &identity)? {
                return Ok(None);
            }
            let projection = ActionProjection::new(proof.clone(), path, owner).map_err(fail)?;
            let mut action = pending.clone();
            action.checked_action = Some(projection.target().clone());
            action.expr = None;
            action.default_boundary = None;
            action.forwarded_call = None;
            for arm in &handler.handlers {
                if owner_ctx.handler_matches_perform(
                    &action,
                    arm,
                    Some(&handler.frame),
                    memory_target.as_ref(),
                )? {
                    return Ok(Some((projection, action)));
                }
            }
            return Ok(None);
        }
        if seen.insert(Arc::as_ptr(&cursor)) {
            for edge in edges
                .iter()
                .rev()
                .filter(|edge| Arc::ptr_eq(edge.source(), &cursor))
            {
                if let Some(mapped) = edge.corresponding_action(&identity) {
                    let mut next = path.clone();
                    next.push(edge.clone());
                    routes.push((edge.target().clone(), mapped, next));
                }
            }
        }
    }
    Ok(None)
}

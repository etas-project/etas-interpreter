use std::{cell::RefCell, rc::Rc, sync::Arc};

use etas_core::{AnalysisDiagnosticCode, Span};

use super::{
    EvalMachine, MachinePoll,
    frame::EvalFrame,
    state::{MachineInput, ProgramScope},
};
use crate::{
    control::{Continuation, ControlSignal, ExecutionFault, PendingPerform, PendingTaskControl},
    driver::scheduler::{RootTaskWait, TaskHandle},
    eval::EvalContext,
    orchestration::{ActiveHandlerRecord, HandlerScopeId, RetryAttemptRecord},
    plan::code::ActionProjection,
    value::{CheckedExecutionProgram, InterpValue},
};

pub(crate) mod checkpoint;

/// The scheduler owns the child machine; this single-shot segment owns only
/// the caller's suspended wait and the frames above its lexical handler.
#[derive(Clone, Debug)]
pub struct TaskHandlerContinuation(Rc<RefCell<Option<SuspendedTaskHandler>>>);

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
                    sites.push((index, scope.clone(), handler.clone()));
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
            let mut cursor = proof.program.clone();
            let mut path = Vec::new();
            for edge in &edges {
                if Arc::ptr_eq(&cursor, &owner.checked) {
                    break;
                }
                if Arc::ptr_eq(edge.source(), &cursor) {
                    cursor = edge.target().clone();
                    path.push(edge.clone());
                }
            }
            if !Arc::ptr_eq(&cursor, &owner.checked) {
                continue;
            }
            let projection = ActionProjection::new(proof.clone(), path, owner).map_err(fail)?;
            let mut action = pending.clone();
            action.checked_action = Some(projection.target().clone());
            action.expr = None;
            action.default_boundary = None;
            action.forwarded_call = None;
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
            let matched = {
                let owner_ctx = ctx.scoped_context(owner, floor);
                let mut matched = false;
                for arm in &handler.handlers {
                    if owner_ctx.handler_matches_perform(&action, arm, Some(&handler.frame))? {
                        matched = true;
                        break;
                    }
                }
                matched
            };
            if !matched {
                continue;
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
                projection,
                calls: TaskCallLease {
                    total: self.active_call_depth.clone(),
                    count,
                },
                span: pending.span,
            };
            action.continuation = Continuation::TaskResume(TaskHandlerContinuation(Rc::new(
                RefCell::new(Some(saved)),
            )));
            self.program_scope = scope;
            self.resync_residual_mirror();
            self.input = Some(MachineInput::Signal(ControlSignal::pending_perform(action)));
            return Ok(Some(handler.scope_id));
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
        let value = match saved
            .projection
            .resume(value, &ctx.checked_execution_program())
        {
            Ok(value) => value,
            Err(reason) => return fail(reason),
        };
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

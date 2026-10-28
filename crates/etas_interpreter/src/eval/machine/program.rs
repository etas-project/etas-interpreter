use std::sync::Arc;

use super::{
    EvalMachine, MachinePoll,
    frame::{EvalFrame, ProgramReturnFrame},
    scoped_call::ProgramCall,
    state::{MachineInput, ProgramScope},
};
use crate::{
    control::{
        Continuation, ContinuationInput, ControlSignal, ExecutionFault, PendingContinuation,
        PendingPerform,
    },
    eval::EvalContext,
    value::InterpValue,
};
use etas_core::{AnalysisDiagnosticCode, Span};

impl EvalMachine {
    pub(super) fn enter_program(
        &mut self,
        ctx: &EvalContext<'_>,
        call: ProgramCall,
        args: Vec<InterpValue>,
        span: Span,
        outer: Continuation,
        site: Option<crate::control::CallSite>,
    ) -> Result<MachinePoll, ExecutionFault> {
        let missing =
            |reason| ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, reason);
        let destination = call.destination().map_err(missing)?;
        let source = self.program_scope.as_ref().map_or_else(
            || ctx.checked_execution_program(),
            |scope| scope.program.clone(),
        );
        let args = call.arguments(&source, args).map_err(missing)?;
        if self.checkpoint_root.is_none() {
            self.checkpoint_root = Some(super::snapshot::CheckpointRoot {
                program: source.clone(),
                args: ctx.entry_args.to_vec(),
            });
        }
        let handler_floor = ctx.handler_stack.len();
        let parent = self.program_scope.clone();
        self.push_frame(EvalFrame::ProgramReturn(Box::new(ProgramReturnFrame {
            site,
            parent,
            parent_program: source,
            parent_model_policy: ctx.model_policy.clone(),
            call: call.clone(),
            outer,
            span,
            retry_depth: ctx.retry_stack.len(),
        })));
        self.program_scope = Some(ProgramScope {
            program: destination,
            handler_floor,
        });
        self.input = Some(MachineInput::EnterProgram { call, args, span });
        Ok(MachinePoll::ProgramChanged)
    }

    pub(super) fn start_program_body(
        &mut self,
        ctx: &mut EvalContext<'_>,
        call: ProgramCall,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let destination = match call.destination() {
            Ok(destination) => destination,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        if !Arc::ptr_eq(ctx.checked, &destination.checked)
            || !std::ptr::eq(ctx.plan, destination.plan.as_ref())
        {
            return ControlSignal::missing_checked_fact(
                "scoped callable is executing under a different checked program or entry plan",
                span,
            );
        }
        match call.body() {
            Ok(target) => ctx.execute_call_target_frame(target, args, span, None),
            Err(reason) => ControlSignal::missing_checked_fact(reason, span),
        }
    }

    pub(super) fn return_program(
        &mut self,
        ctx: &mut EvalContext<'_>,
        frame: ProgramReturnFrame,
        value: InterpValue,
    ) -> MachinePoll {
        let fail = |reason| {
            MachinePoll::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                frame.span,
                reason,
            ))
        };
        let program = match self.checked_program_exit(ctx, &frame) {
            Ok(program) => program,
            Err(reason) => return fail(reason),
        };
        let value = match frame.call.result(program, value) {
            Ok(value) => value,
            Err(reason) => return fail(reason),
        };
        ctx.model_policy = frame.parent_model_policy;
        self.program_scope = frame.parent;
        self.input = Some(MachineInput::Signal(ControlSignal::pending_continuation(
            PendingContinuation {
                continuation: frame.outer,
                input: ContinuationInput::Value(value),
            },
        )));
        MachinePoll::ProgramChanged
    }

    pub(super) fn unwind_program_error(
        &mut self,
        ctx: &mut EvalContext<'_>,
        frame: ProgramReturnFrame,
        pending: PendingPerform,
    ) -> MachinePoll {
        let fail = |reason| {
            MachinePoll::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                frame.span,
                reason,
            ))
        };
        let program = match self.checked_program_exit(ctx, &frame) {
            Ok(program) => program,
            Err(reason) => return fail(reason),
        };
        let Some(ty) = pending.error_type else {
            return fail("resumable action requires a program-aware continuation".into());
        };
        let [payload] = pending.args.as_slice() else {
            return fail("typed error has no single checked payload".into());
        };
        let (ty, payload) = match frame.call.error(program, ty, payload.clone()) {
            Ok(projected) => projected,
            Err(reason) => return fail(reason),
        };
        // Error.raise returns never. Its abandoned native continuation cannot be
        // resumed, and no source-program HIR IDs may enter the parent scope.
        let error = PendingPerform::typed_error(ty, payload, frame.span, Continuation::BlockValue);
        self.push_continuation_frames(frame.outer);
        ctx.model_policy = frame.parent_model_policy;
        self.program_scope = frame.parent;
        self.input = Some(MachineInput::Signal(ControlSignal::pending_perform(error)));
        MachinePoll::ProgramChanged
    }

    fn checked_program_exit(
        &self,
        ctx: &EvalContext<'_>,
        frame: &ProgramReturnFrame,
    ) -> Result<&crate::value::CheckedExecutionProgram, String> {
        let destination = frame.call.destination()?;
        let scope = self
            .program_scope
            .as_ref()
            .ok_or("program exit has no active checked execution scope")?;
        if !Arc::ptr_eq(&scope.program.checked, &destination.checked)
            || !Arc::ptr_eq(&scope.program.plan, &destination.plan)
            || !Arc::ptr_eq(ctx.checked, &destination.checked)
            || ctx.handler_floor != scope.handler_floor
            || ctx.handler_stack.len() != ctx.handler_floor
            || ctx.retry_stack.len() != frame.retry_depth
        {
            return Err(
                "scoped exit disagrees with its program or dynamic handler/retry scope".into(),
            );
        }
        Ok(&scope.program)
    }
}

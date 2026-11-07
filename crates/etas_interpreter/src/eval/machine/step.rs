use etas_core::AnalysisDiagnosticCode;

use crate::{
    control::{
        Continuation, ContinuationInput, ControlSignal, ExecutionFault, residual::ResidualExitKind,
    },
    eval::EvalContext,
    eval::machine::{
        budget::check_call_budget,
        frame::{BlockFrame, CallFrame, EvalFrame, ExprFrame},
        state::{EvalMachine, MachineInput, MachinePoll, PendingBoundary},
    },
};

#[cfg(test)]
mod tests;

macro_rules! local_continuation {
    ($frame:expr, $span:expr) => {
        match $frame.into_continuation() {
            Ok(continuation) => continuation,
            Err(reason) => return machine_fault(reason, $span),
        }
    };
}

impl EvalMachine {
    fn poll_native_input(
        &mut self,
        ctx: &mut EvalContext<'_>,
        mut pending: Box<crate::intrinsic::pure::PendingNativeWork>,
        step_span: etas_core::Span,
    ) -> Option<ControlSignal> {
        let call = pending.call().cloned();
        match pending.poll(ctx.plan.dispatch.pure_abi()) {
            Ok(crate::intrinsic::pure::NativeWorkPoll::Ready(value)) => {
                Some(ControlSignal::Value(value))
            }
            Ok(crate::intrinsic::pure::NativeWorkPoll::IterationReady(value)) => {
                let Some((state, source, mut frame)) = pending.into_iteration() else {
                    return Some(ControlSignal::missing_checked_fact(
                        "native iteration completed without loop state",
                        step_span,
                    ));
                };
                Some(ctx.resume_native_for_loop(state, source, value, &mut frame))
            }
            Ok(crate::intrinsic::pure::NativeWorkPoll::Pending) => {
                self.input = Some(MachineInput::Signal(ControlSignal::Native(pending)));
                None
            }
            Err(error) => Some(match call {
                Some(call) => ctx.complete_pure_intrinsic(&call, Err(error), step_span),
                None => ControlSignal::missing_checked_fact(
                    format!("native text iteration failed: {error:?}"),
                    step_span,
                ),
            }),
        }
    }

    pub(crate) fn has_native_input(&self) -> bool {
        matches!(
            self.input,
            Some(MachineInput::Signal(ControlSignal::Native(_)))
        )
    }

    /// Complete the admitted pure kernel without polling language continuations.
    pub(crate) async fn drain_native_input(
        &mut self,
        ctx: &mut EvalContext<'_>,
    ) -> Option<etas_host::execution::CancellationCause> {
        let span = crate::diagnostics::item_span(ctx.checked, ctx.entry_item);
        while self.has_native_input() {
            if let Some(signal) = ctx.cancellation_signal(span) {
                match signal {
                    ControlSignal::Cancelled(cause) => return Some(cause),
                    signal => {
                        self.resume(signal);
                        return None;
                    }
                }
            }
            if let Err(fault) = ctx.consume_execution_step(span) {
                self.resume(ControlSignal::Fault(Box::new(fault)));
                return None;
            }
            let Some(MachineInput::Signal(ControlSignal::Native(pending))) = self.input.take()
            else {
                unreachable!("native input is owned by this machine")
            };
            if let Some(signal) = self.poll_native_input(ctx, pending, span) {
                self.resume(signal);
            } else {
                tokio::task::yield_now().await;
            }
        }
        None
    }
    pub(crate) fn run_until_yield(&mut self, ctx: &mut EvalContext<'_>) -> MachinePoll {
        let step_span = crate::diagnostics::item_span(ctx.checked, ctx.entry_item);
        if let Some(scope) = &self.program_scope
            && (!std::sync::Arc::ptr_eq(ctx.checked, scope.program())
                || ctx.handler_floor != scope.handler_floor)
        {
            return machine_fault(
                "evaluation machine revision disagrees with the active checked program scope",
                step_span,
            );
        }
        // A previously produced fault remains primary if stop races its delivery.
        if let Some(MachineInput::Signal(ControlSignal::Fault(_))) = &self.input {
            if let Some(MachineInput::Signal(ControlSignal::Fault(fault))) = self.input.take() {
                return MachinePoll::Fault(*fault);
            }
        }
        if let Some(poll) = scheduling_poll(ctx.scheduling_decision(step_span)) {
            if !matches!(poll, MachinePoll::CooperativeYield)
                || !matches!(self.input.as_ref(), Some(MachineInput::Signal(signal)) if !durable_yield_input(signal))
            {
                return poll;
            }
        }
        let mirror = ctx.task.residual.mirror.clone();
        self.join_residual_mirror(&mirror);
        let Some(input) = self.input.take() else {
            return machine_fault(
                "evaluation machine was polled without a start or resume input".to_owned(),
                crate::diagnostics::item_span(ctx.checked, ctx.entry_item),
            );
        };
        let mut signal = match input {
            MachineInput::EnterProgram { call, args, span } => {
                self.start_program_body(ctx, call, args, span)
            }
            MachineInput::Signal(signal) => signal,
            MachineInput::ModelResult(result) => match self.resume_model_host_result(ctx, result) {
                Ok(signal) => signal,
                Err(poll) => return poll,
            },
            MachineInput::ToolResult(result) => match self.resume_tool_host_result(ctx, result) {
                Ok(signal) => signal,
                Err(poll) => return poll,
            },
            MachineInput::SourceToolApproved => match self.resume_source_tool_approved_input(ctx) {
                Ok(signal) => signal,
                Err(poll) => return poll,
            },
        };
        loop {
            if let ControlSignal::Fault(fault) = signal {
                return MachinePoll::Fault(*fault);
            }
            if let Some(poll) = scheduling_poll(ctx.scheduling_decision(step_span)) {
                if !matches!(poll, MachinePoll::CooperativeYield) || durable_yield_input(&signal) {
                    self.input = Some(MachineInput::Signal(signal));
                    return poll;
                }
            }
            if let Some(span) = signal.requested_operation_span(step_span)
                && self
                    .stack
                    .iter()
                    .any(|frame| matches!(frame, EvalFrame::PureBoundary(_)))
            {
                return MachinePoll::Fault(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    "pure comparison cannot request effects or Host operations",
                ));
            }
            if let Err(fault) = ctx.consume_execution_step(step_span) {
                return MachinePoll::Fault(fault);
            }
            signal = match signal {
                ControlSignal::Apply(pending) => {
                    let pending = *pending;
                    let Some(first) = self.push_outer_continuation_frames(pending.continuation)
                    else {
                        return machine_fault(
                            "evaluation machine received an empty continuation sequence".to_owned(),
                            crate::diagnostics::item_span(ctx.checked, ctx.entry_item),
                        );
                    };
                    match pending.input {
                        ContinuationInput::Value(value) => {
                            if let Continuation::ProgramResume(continuation) = first {
                                return self.resume_program_continuation(ctx, continuation, value);
                            }
                            if let Continuation::SourceToolReturn(saved) = first {
                                match self.resume_source_tool_value(ctx, *saved.0, value) {
                                    Ok(signal) => signal,
                                    Err(poll) => return poll,
                                }
                            } else {
                                ctx.apply_continuation_frame(first, value)
                            }
                        }
                        ContinuationInput::Return(value) => {
                            if let Continuation::SourceToolReturn(saved) = first {
                                match self.resume_source_tool_value(ctx, *saved.0, value) {
                                    Ok(signal) => signal,
                                    Err(poll) => return poll,
                                }
                            } else {
                                ctx.propagate_return_frame(value, first)
                            }
                        }
                        ContinuationInput::Resume(value) => {
                            ctx.propagate_resume_frame(value, first)
                        }
                        ContinuationInput::Finish(value) => {
                            ctx.propagate_finish_frame(value, first)
                        }
                        ContinuationInput::Break => {
                            ctx.propagate_loop_control_frame(ControlSignal::Break, first)
                        }
                        ContinuationInput::Continue => {
                            ctx.propagate_loop_control_frame(ControlSignal::Continue, first)
                        }
                    }
                }
                ControlSignal::Checkpoint(pending) => {
                    let pending = *pending;
                    self.push_continuation_frames(pending.continuation);
                    self.resume(ControlSignal::Value(crate::value::InterpValue::Unit));
                    return MachinePoll::Yield(PendingBoundary::Checkpoint {
                        label: pending.label,
                        span: step_span,
                    });
                }
                ControlSignal::Block(pending) => {
                    let mut pending = *pending;
                    self.push_continuation_frames(pending.continuation);
                    if let Some(activation) = pending.residual.take() {
                        match ctx.activate_residual_scope(*activation, step_span) {
                            Ok(scope) => {
                                let invocation = crate::eval::residual::invocation_counter(
                                    scope.borrow().invocation().as_bytes(),
                                );
                                if let Err(reason) = invocation.and_then(|invocation| {
                                    pending.frame.bind_residual_invocation(invocation)
                                }) {
                                    return machine_fault(reason, step_span);
                                }
                                self.push_frame(EvalFrame::ResidualScope(scope));
                            }
                            Err(fault) => return MachinePoll::Fault(fault),
                        }
                    }
                    self.push_frame(EvalFrame::Block(BlockFrame {
                        continuation: Continuation::BlockValue,
                    }));
                    ctx.execute_block_frame(
                        pending.block,
                        pending.next_stmt_index,
                        &mut pending.frame,
                    )
                }
                ControlSignal::Expr(pending) => {
                    let mut pending = *pending;
                    self.push_continuation_frames(pending.continuation);
                    self.push_frame(EvalFrame::Expr(ExprFrame {
                        continuation: if ctx
                            .checked
                            .types
                            .existential_packs
                            .contains_key(&pending.expr)
                        {
                            Continuation::PackExistential {
                                expr: pending.expr,
                                frame: pending.frame.clone(),
                            }
                        } else {
                            Continuation::BlockValue
                        },
                    }));
                    ctx.eval_expr_frame(pending.expr, &mut pending.frame)
                }
                ControlSignal::Call(pending) => {
                    if let Err(fault) = check_call_budget(ctx, self, pending.span) {
                        return MachinePoll::Fault(fault);
                    }
                    let pending = *pending;
                    match ctx.prepare_call_target(pending.target, pending.continuation) {
                        Ok((target, mut continuation)) => {
                            if pending.purity == crate::control::CallPurity::Pure {
                                self.push_continuation_frames(continuation);
                                self.push_frame(EvalFrame::PureBoundary(pending.span));
                                continuation = Continuation::BlockValue;
                            }
                            let target = match super::scoped_call::ProgramCall::select(
                                target,
                                ctx.checked,
                            ) {
                                Ok(Ok(call)) => Err(call),
                                Ok(Err(target)) => Ok(target),
                                Err(reason) => return machine_fault(reason, pending.span),
                            };
                            let target = match target {
                                Err(call) => {
                                    return self
                                        .enter_program(
                                            ctx,
                                            call,
                                            pending.args,
                                            pending.span,
                                            continuation,
                                            pending.site,
                                        )
                                        .unwrap_or_else(MachinePoll::Fault);
                                }
                                Ok(target) => target,
                            };
                            self.push_continuation_frames(continuation);
                            self.push_frame(EvalFrame::Call(CallFrame {
                                caller: pending
                                    .site
                                    .as_ref()
                                    .map(crate::control::CallOrigin::from_site),
                                continuation: Continuation::BlockValue,
                                span: pending.span,
                            }));
                            ctx.execute_call_target_frame(
                                target,
                                pending.args,
                                pending.span,
                                pending.site,
                            )
                        }
                        Err(signal) => *signal,
                    }
                }
                ControlSignal::Native(pending) => {
                    match self.poll_native_input(ctx, pending, step_span) {
                        Some(signal) => signal,
                        None => return MachinePoll::CooperativeYield,
                    }
                }
                ControlSignal::Value(value) => {
                    let Some(frame) = self.pop_frame() else {
                        return MachinePoll::Complete(Box::new(value));
                    };
                    match frame {
                        EvalFrame::PureBoundary(_) => ControlSignal::Value(value),
                        EvalFrame::ResidualScope(scope) => {
                            match ctx.exit_residual_scope(
                                &scope,
                                ResidualExitKind::Return,
                                step_span,
                            ) {
                                Ok(()) => ControlSignal::Value(value),
                                Err(fault) => return MachinePoll::Fault(fault),
                            }
                        }
                        EvalFrame::ProgramReturn(frame) => {
                            return self.return_program(ctx, *frame, value);
                        }
                        EvalFrame::Block(frame) => ctx
                            .continue_chain_signal(ControlSignal::Value(value), frame.continuation),
                        EvalFrame::Call(frame) => ctx.apply_continuation(frame.continuation, value),
                        EvalFrame::Continuation(frame) => {
                            ctx.apply_continuation(frame.continuation, value)
                        }
                        EvalFrame::Expr(frame) => ctx.apply_continuation(frame.continuation, value),
                        frame @ (EvalFrame::Handler(_) | EvalFrame::Retry(_)) => {
                            ctx.apply_continuation(local_continuation!(frame, step_span), value)
                        }
                        EvalFrame::SourceToolReturn(frame) => {
                            match self.resume_source_tool_value(ctx, frame, value) {
                                Ok(signal) => signal,
                                Err(poll) => return poll,
                            }
                        }
                        EvalFrame::ModelLoop(frame) => {
                            return machine_fault("model loop received an evaluator value without a model/tool response".to_owned(), frame.pending.span);
                        }
                    }
                }
                ControlSignal::Return(value) => {
                    let Some(frame) = self.pop_frame() else {
                        return MachinePoll::Complete(Box::new(value));
                    };
                    match frame {
                        EvalFrame::PureBoundary(_) => ControlSignal::Return(value),
                        EvalFrame::ResidualScope(scope) => {
                            match ctx.exit_residual_scope(
                                &scope,
                                ResidualExitKind::Return,
                                step_span,
                            ) {
                                Ok(()) => ControlSignal::Return(value),
                                Err(fault) => return MachinePoll::Fault(fault),
                            }
                        }
                        EvalFrame::ProgramReturn(frame) => {
                            return self.return_program(ctx, *frame, value);
                        }
                        EvalFrame::Block(frame) => ctx.continue_chain_signal(
                            ControlSignal::Return(value),
                            frame.continuation,
                        ),
                        EvalFrame::Call(frame) => ctx.apply_continuation(frame.continuation, value),
                        EvalFrame::Continuation(frame) => ctx.continue_chain_signal(
                            ControlSignal::Return(value),
                            frame.continuation,
                        ),
                        EvalFrame::Expr(_) => ControlSignal::Return(value),
                        frame @ (EvalFrame::Handler(_) | EvalFrame::Retry(_)) => ctx
                            .continue_chain_signal(
                                ControlSignal::Return(value),
                                local_continuation!(frame, step_span),
                            ),
                        EvalFrame::SourceToolReturn(frame) => {
                            match self.resume_source_tool_value(ctx, frame, value) {
                                Ok(signal) => signal,
                                Err(poll) => return poll,
                            }
                        }
                        EvalFrame::ModelLoop(frame) => {
                            return machine_fault("model loop received a source return without an active source tool frame".to_owned(), frame.pending.span);
                        }
                    }
                }
                ControlSignal::Perform(mut pending) => {
                    // Observed at first sight, before any frame is popped, so
                    // every enclosing residual scope previews the attempt.
                    if let Err(fault) = ctx.observe_residual_attempt(&mut pending) {
                        return MachinePoll::Fault(fault);
                    }
                    // Default handling suspends the current stack; it does not exit
                    // unrelated lexical scopes. Typed errors still visit try frames.
                    if pending.error_type.is_none() {
                        match self.has_action_handler_in(
                            ctx,
                            &pending,
                            ctx.handler_floor..ctx.task.handler_stack.len(),
                        ) {
                            Ok(false) => {
                                match self.has_outer_action_handler(ctx, &pending) {
                                    Ok(true) => return self.suspend_program_action(ctx, *pending),
                                    Ok(false) => {}
                                    Err(fault) => return MachinePoll::Fault(fault),
                                }
                                if pending.default_boundary.is_some() {
                                    signal = ctx.execute_checked_default_action(*pending);
                                    continue;
                                }
                                return MachinePoll::Yield(PendingBoundary::Perform(pending));
                            }
                            Ok(true) => {}
                            Err(fault) => return MachinePoll::Fault(fault),
                        }
                    }
                    let Some(frame) = self.pop_frame() else {
                        return MachinePoll::Yield(PendingBoundary::Perform(pending));
                    };
                    let continuation = match frame {
                        EvalFrame::PureBoundary(span) => {
                            return machine_fault(
                                "effect request crossed a pure comparison boundary",
                                span,
                            );
                        }
                        // A typed error unwinds the scope; an action request
                        // handled outside it captures the scope instead.
                        EvalFrame::ResidualScope(scope) if pending.error_type.is_some() => {
                            if let Err(fault) = ctx.exit_residual_scope(
                                &scope,
                                ResidualExitKind::ErrorPropagation,
                                pending.span,
                            ) {
                                return MachinePoll::Fault(fault);
                            }
                            Continuation::BlockValue
                        }
                        EvalFrame::ResidualScope(scope) => Continuation::ResidualScope(scope),
                        EvalFrame::ProgramReturn(frame) => {
                            return self.unwind_program_error(ctx, *frame, *pending);
                        }
                        EvalFrame::Block(frame) => frame.continuation,
                        EvalFrame::Call(frame) => Continuation::CallBoundary {
                            caller: frame.caller,
                            span: frame.span,
                            outer: frame.continuation.into(),
                        },
                        EvalFrame::Continuation(frame) => frame.continuation,
                        EvalFrame::Expr(frame) => frame.continuation,
                        frame @ (EvalFrame::Handler(_) | EvalFrame::Retry(_)) => {
                            local_continuation!(frame, step_span)
                        }
                        EvalFrame::SourceToolReturn(frame) => Continuation::SourceToolReturn(
                            super::SourceToolContinuation(Box::new(frame)),
                        ),
                        EvalFrame::ModelLoop(frame) => {
                            return machine_fault(
                                "perform crossed an incomplete model loop".to_owned(),
                                frame.pending.span,
                            );
                        }
                    };
                    pending.continuation =
                        crate::eval::compose_continuation(pending.continuation, continuation);
                    ctx.propagate_perform_signal(*pending)
                }
                ControlSignal::Memory(pending) => {
                    return MachinePoll::Yield(PendingBoundary::Memory(pending));
                }
                ControlSignal::Session(pending) => {
                    return MachinePoll::Yield(PendingBoundary::Session(pending));
                }
                ControlSignal::Console(pending) => {
                    return MachinePoll::Yield(PendingBoundary::Console(pending));
                }
                ControlSignal::Command(pending) => {
                    return MachinePoll::Yield(PendingBoundary::Command(pending));
                }
                ControlSignal::Model(pending) => match self.begin_model(ctx, *pending) {
                    Ok(signal) => signal,
                    Err(poll) => return poll,
                },
                ControlSignal::Host(pending) => {
                    return MachinePoll::Yield(PendingBoundary::Host(pending));
                }
                ControlSignal::Task(pending) => {
                    return MachinePoll::Yield(PendingBoundary::Task(pending));
                }
                ControlSignal::Fault(fault) => return MachinePoll::Fault(*fault),
                ControlSignal::Cancelled(cause) => return MachinePoll::Cancelled(cause),
                ControlSignal::Resume(value) => {
                    let Some(frame) = self.pop_frame() else {
                        return MachinePoll::Complete(Box::new(value));
                    };
                    match frame {
                        EvalFrame::PureBoundary(_) => ControlSignal::Resume(value),
                        EvalFrame::ResidualScope(scope) => {
                            match ctx.exit_residual_scope(
                                &scope,
                                ResidualExitKind::NonLocalReturn,
                                step_span,
                            ) {
                                Ok(()) => ControlSignal::Resume(value),
                                Err(fault) => return MachinePoll::Fault(fault),
                            }
                        }
                        EvalFrame::ProgramReturn(frame) => {
                            return machine_fault(
                                "resume escaped a revision call boundary",
                                frame.span,
                            );
                        }
                        EvalFrame::Block(frame) => ctx.continue_chain_signal(
                            ControlSignal::Resume(value),
                            frame.continuation,
                        ),
                        EvalFrame::Call(frame) => ctx.apply_continuation(frame.continuation, value),
                        EvalFrame::Continuation(frame) => ctx.continue_chain_signal(
                            ControlSignal::Resume(value),
                            frame.continuation,
                        ),
                        EvalFrame::Expr(_) => ControlSignal::Resume(value),
                        frame @ (EvalFrame::Handler(_) | EvalFrame::Retry(_)) => ctx
                            .continue_chain_signal(
                                ControlSignal::Resume(value),
                                local_continuation!(frame, step_span),
                            ),
                        EvalFrame::SourceToolReturn(frame) => {
                            return machine_fault(
                                "resume escaped a source tool execution frame".to_owned(),
                                frame.model_loop.pending.span,
                            );
                        }
                        EvalFrame::ModelLoop(frame) => {
                            return machine_fault(
                                "resume escaped into an incomplete model loop".to_owned(),
                                frame.pending.span,
                            );
                        }
                    }
                }
                ControlSignal::Finish(value) => {
                    let Some(frame) = self.pop_frame() else {
                        return MachinePoll::Complete(Box::new(value));
                    };
                    match frame {
                        EvalFrame::PureBoundary(_) => ControlSignal::Finish(value),
                        EvalFrame::ResidualScope(scope) => {
                            match ctx.exit_residual_scope(
                                &scope,
                                ResidualExitKind::HandlerFinish,
                                step_span,
                            ) {
                                Ok(()) => ControlSignal::Finish(value),
                                Err(fault) => return MachinePoll::Fault(fault),
                            }
                        }
                        EvalFrame::ProgramReturn(frame) => {
                            return machine_fault(
                                "finish escaped a revision call boundary",
                                frame.span,
                            );
                        }
                        EvalFrame::Block(frame) => ctx.continue_chain_signal(
                            ControlSignal::Finish(value),
                            frame.continuation,
                        ),
                        EvalFrame::Call(frame) => ctx.apply_continuation(frame.continuation, value),
                        EvalFrame::Continuation(frame) => ctx.continue_chain_signal(
                            ControlSignal::Finish(value),
                            frame.continuation,
                        ),
                        EvalFrame::Expr(_) => ControlSignal::Finish(value),
                        frame @ (EvalFrame::Handler(_) | EvalFrame::Retry(_)) => ctx
                            .continue_chain_signal(
                                ControlSignal::Finish(value),
                                local_continuation!(frame, step_span),
                            ),
                        EvalFrame::SourceToolReturn(frame) => {
                            return machine_fault(
                                "finish escaped a source tool execution frame".to_owned(),
                                frame.model_loop.pending.span,
                            );
                        }
                        EvalFrame::ModelLoop(frame) => {
                            return machine_fault(
                                "finish escaped into an incomplete model loop".to_owned(),
                                frame.pending.span,
                            );
                        }
                    }
                }
                signal @ (ControlSignal::Break | ControlSignal::Continue) => {
                    let Some(frame) = self.pop_frame() else {
                        return machine_fault(
                            "loop control escaped its execution frame".to_owned(),
                            crate::diagnostics::item_span(ctx.checked, ctx.entry_item),
                        );
                    };
                    match frame {
                        EvalFrame::PureBoundary(_) => signal,
                        EvalFrame::ResidualScope(scope) => {
                            match ctx.exit_residual_scope(
                                &scope,
                                ResidualExitKind::NonLocalReturn,
                                step_span,
                            ) {
                                Ok(()) => signal,
                                Err(fault) => return MachinePoll::Fault(fault),
                            }
                        }
                        EvalFrame::ProgramReturn(frame) => {
                            return machine_fault(
                                "loop control escaped a revision call boundary",
                                frame.span,
                            );
                        }
                        EvalFrame::Block(frame) => {
                            ctx.continue_chain_signal(signal, frame.continuation)
                        }
                        EvalFrame::Expr(frame) => {
                            ctx.propagate_loop_control_to_continuation(signal, frame.continuation)
                        }
                        EvalFrame::Continuation(frame) => {
                            ctx.propagate_loop_control_to_continuation(signal, frame.continuation)
                        }
                        frame @ (EvalFrame::Handler(_) | EvalFrame::Retry(_)) => ctx
                            .propagate_loop_control_to_continuation(
                                signal,
                                local_continuation!(frame, step_span),
                            ),
                        EvalFrame::Call(frame) => {
                            return machine_fault(
                                "loop control escaped its execution frame".to_owned(),
                                frame.span,
                            );
                        }
                        EvalFrame::SourceToolReturn(frame) => {
                            return machine_fault(
                                "loop control escaped a source tool execution frame".to_owned(),
                                frame.model_loop.pending.span,
                            );
                        }
                        EvalFrame::ModelLoop(frame) => {
                            return machine_fault(
                                "loop control escaped into an incomplete model loop".to_owned(),
                                frame.pending.span,
                            );
                        }
                    }
                }
            };
        }
    }

    pub(super) fn push_continuation_frames(&mut self, continuation: Continuation) {
        visit_continuation_stack_order(continuation, |continuation| {
            if !matches!(continuation, Continuation::BlockValue) {
                self.push_frame(EvalFrame::from_continuation(continuation));
            }
        });
    }

    fn push_outer_continuation_frames(
        &mut self,
        continuation: Continuation,
    ) -> Option<Continuation> {
        let mut first = None;
        visit_continuation_stack_order(continuation, |continuation| {
            if let Some(outer) = first.replace(continuation) {
                self.push_frame(EvalFrame::from_continuation(outer));
            }
        });
        first
    }
}

// Transient native/effect dispatch reaches a durable input or a Host boundary
// before yielding. A checkpoint never has to persist an unissued Host request.
fn durable_yield_input(signal: &ControlSignal) -> bool {
    !matches!(
        signal,
        ControlSignal::Native(_)
            | ControlSignal::Perform(_)
            | ControlSignal::Memory(_)
            | ControlSignal::Session(_)
            | ControlSignal::Console(_)
            | ControlSignal::Command(_)
            | ControlSignal::Model(_)
            | ControlSignal::Host(_)
            | ControlSignal::Cancelled(_)
    )
}

fn machine_fault(message: impl Into<String>, span: etas_core::Span) -> MachinePoll {
    MachinePoll::Fault(ExecutionFault::new(
        AnalysisDiagnosticCode::UnhandledRuntimeError,
        span,
        message,
    ))
}

// Visit outer leaves first so they can go directly onto the machine stack.
// A leaf needs no scratch allocation; only unvisited branch links are retained.
fn visit_continuation_stack_order(
    mut continuation: Continuation,
    mut visit: impl FnMut(Continuation),
) {
    let mut pending = Vec::new();
    loop {
        match continuation {
            Continuation::Chain { inner, outer } => {
                pending.push(inner);
                continuation = outer.into_value();
                continue;
            }
            leaf => visit(leaf),
        }
        let Some(next) = pending.pop() else { break };
        continuation = next.into_value();
    }
}

fn scheduling_poll(decision: crate::eval::safe_point::SafePointDecision) -> Option<MachinePoll> {
    use crate::eval::safe_point::SafePointDecision;
    match decision {
        SafePointDecision::Continue => None,
        SafePointDecision::Yield => Some(MachinePoll::CooperativeYield),
        SafePointDecision::Cancelled(cause) => Some(MachinePoll::Cancelled(cause)),
        SafePointDecision::Fault(fault) => Some(MachinePoll::Fault(fault)),
    }
}

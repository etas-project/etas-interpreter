use etas_core::AnalysisDiagnosticCode;

use crate::{
    control::{Continuation, ContinuationInput, ControlSignal, ExecutionFault},
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
            return poll;
        }
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
                self.input = Some(MachineInput::Signal(signal));
                return poll;
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
                            ctx.apply_continuation_frame(first, value)
                        }
                        ContinuationInput::Return(value) => {
                            ctx.propagate_return_frame(value, first)
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
                    let machine = match self.capture_snapshot(ctx.entry_args) {
                        Ok(machine) => machine,
                        Err(message) => {
                            return machine_fault(
                                format!("checkpoint cannot serialize evaluation stack: {message}"),
                                crate::diagnostics::item_span(ctx.checked, ctx.entry_item),
                            );
                        }
                    };
                    if let Err(fault) = ctx.record_checkpoint(pending.label, machine) {
                        return MachinePoll::Fault(fault);
                    }
                    ControlSignal::Value(crate::value::InterpValue::Unit)
                }
                ControlSignal::Block(pending) => {
                    let mut pending = *pending;
                    self.push_continuation_frames(pending.continuation);
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
                        continuation: Continuation::BlockValue,
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
                ControlSignal::Value(value) => {
                    let Some(frame) = self.pop_frame() else {
                        return MachinePoll::Complete(Box::new(value));
                    };
                    match frame {
                        EvalFrame::PureBoundary(_) => ControlSignal::Value(value),
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
                    // Default handling suspends the current stack; it does not exit
                    // unrelated lexical scopes. Typed errors still visit try frames.
                    if pending.error_type.is_none() {
                        match self.has_action_handler_in(
                            ctx,
                            &pending,
                            ctx.handler_floor..ctx.handler_stack.len(),
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
                        EvalFrame::ProgramReturn(frame) => {
                            return self.unwind_program_error(ctx, *frame, *pending);
                        }
                        EvalFrame::Block(frame) => frame.continuation,
                        EvalFrame::Call(frame) => Continuation::CallBoundary {
                            span: frame.span,
                            outer: frame.continuation.into(),
                        },
                        EvalFrame::Continuation(frame) => frame.continuation,
                        EvalFrame::Expr(frame) => frame.continuation,
                        frame @ (EvalFrame::Handler(_) | EvalFrame::Retry(_)) => {
                            local_continuation!(frame, step_span)
                        }
                        EvalFrame::SourceToolReturn(frame) => {
                            self.push_frame(EvalFrame::SourceToolReturn(frame));
                            Continuation::BlockValue
                        }
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
                ControlSignal::Fault(fault) => return MachinePoll::Fault(*fault),
                ControlSignal::Cancelled(cause) => return MachinePoll::Cancelled(cause),
                ControlSignal::Resume(value) => {
                    let Some(frame) = self.pop_frame() else {
                        return MachinePoll::Complete(Box::new(value));
                    };
                    match frame {
                        EvalFrame::PureBoundary(_) => ControlSignal::Resume(value),
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

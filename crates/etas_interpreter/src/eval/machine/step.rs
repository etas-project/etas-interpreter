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

impl EvalMachine {
    pub(crate) fn run_until_yield(&mut self, ctx: &mut EvalContext<'_>) -> MachinePoll {
        let step_span = crate::diagnostics::item_span(ctx.checked, ctx.entry_item);
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
            if let Err(fault) = ctx.consume_execution_step(step_span) {
                return MachinePoll::Fault(fault);
            }
            signal = match signal {
                ControlSignal::Apply(pending) => {
                    let pending = *pending;
                    let mut continuations = flatten_continuation(pending.continuation);
                    let Some(first) = continuations.next() else {
                        return machine_fault(
                            "evaluation machine received an empty continuation sequence".to_owned(),
                            crate::diagnostics::item_span(ctx.checked, ctx.entry_item),
                        );
                    };
                    let remaining = continuations.collect::<Vec<_>>();
                    for continuation in remaining.into_iter().rev() {
                        self.push_frame(EvalFrame::from_continuation(continuation));
                    }
                    match pending.input {
                        ContinuationInput::Value(value) => {
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
                    let machine = match self.snapshot() {
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
                        Ok((target, continuation)) => {
                            self.push_continuation_frames(continuation);
                            self.push_frame(EvalFrame::Call(CallFrame {
                                continuation: Continuation::BlockValue,
                                span: pending.span,
                            }));
                            ctx.execute_call_target_frame(target, pending.args, pending.span)
                        }
                        Err(signal) => *signal,
                    }
                }
                ControlSignal::Value(value) => {
                    let Some(frame) = self.pop_frame() else {
                        return MachinePoll::Complete(Box::new(value));
                    };
                    match frame {
                        EvalFrame::Block(frame) => ctx
                            .continue_chain_signal(ControlSignal::Value(value), frame.continuation),
                        EvalFrame::Call(frame) => ctx.apply_continuation(frame.continuation, value),
                        EvalFrame::Continuation(frame) => {
                            ctx.apply_continuation(frame.continuation, value)
                        }
                        EvalFrame::Expr(frame) => ctx.apply_continuation(frame.continuation, value),
                        frame @ (EvalFrame::Handler(_) | EvalFrame::Retry(_)) => {
                            ctx.apply_continuation(frame.into_continuation(), value)
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
                                frame.into_continuation(),
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
                    let Some(frame) = self.pop_frame() else {
                        return MachinePoll::Yield(PendingBoundary::Perform(pending));
                    };
                    let continuation = match frame {
                        EvalFrame::Block(frame) => frame.continuation,
                        EvalFrame::Call(frame) => Continuation::CallBoundary {
                            outer: Box::new(frame.continuation),
                        },
                        EvalFrame::Continuation(frame) => frame.continuation,
                        EvalFrame::Expr(frame) => frame.continuation,
                        frame @ (EvalFrame::Handler(_) | EvalFrame::Retry(_)) => {
                            frame.into_continuation()
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
                                frame.into_continuation(),
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
                                frame.into_continuation(),
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
                                frame.into_continuation(),
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

    fn push_continuation_frames(&mut self, continuation: Continuation) {
        let continuations = flatten_continuation(continuation).collect::<Vec<_>>();
        for continuation in continuations.into_iter().rev() {
            if !matches!(continuation, Continuation::BlockValue) {
                self.push_frame(EvalFrame::from_continuation(continuation));
            }
        }
    }
}

fn machine_fault(message: impl Into<String>, span: etas_core::Span) -> MachinePoll {
    MachinePoll::Fault(ExecutionFault::new(
        AnalysisDiagnosticCode::UnhandledRuntimeError,
        span,
        message,
    ))
}

fn flatten_continuation(continuation: Continuation) -> std::vec::IntoIter<Continuation> {
    let mut pending = vec![continuation];
    let mut flattened = Vec::new();
    while let Some(continuation) = pending.pop() {
        match continuation {
            Continuation::Chain { inner, outer } => {
                pending.push(*outer);
                pending.push(*inner);
            }
            continuation => flattened.push(continuation),
        }
    }
    flattened.into_iter()
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

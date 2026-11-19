use etas_core::Span;

use crate::{
    control::{Continuation, ContinuationInput, ControlSignal, PendingContinuation},
    eval::EvalContext,
};

use super::{
    MachinePoll,
    frame::EvalFrame,
    state::{EvalMachine, MachineInput},
};

impl EvalMachine {
    pub(crate) fn resume(&mut self, signal: ControlSignal) {
        assert!(
            self.input.replace(MachineInput::Signal(signal)).is_none(),
            "EvalMachine cannot resume while another signal is pending"
        );
    }

    pub(crate) fn transfer_from(
        &mut self,
        local: &mut crate::eval::TaskLocalState,
        token: crate::value::ContinuationTokenValue,
        continuation: crate::control::Continuation,
        value: crate::value::InterpValue,
    ) {
        self.transfer_signal(
            local,
            token,
            ControlSignal::pending_continuation(PendingContinuation {
                continuation,
                input: ContinuationInput::Value(value),
            }),
        );
    }

    pub(crate) fn transfer_signal(
        &mut self,
        local: &mut crate::eval::TaskLocalState,
        token: crate::value::ContinuationTokenValue,
        signal: ControlSignal,
    ) {
        let boundary = self.stack.iter().rposition(|frame| {
            matches!(
                frame,
                super::frame::EvalFrame::Continuation(frame)
                    if matches!(
                        frame.continuation,
                        crate::control::Continuation::HandlerDispatch {
                            deferred: Some(candidate), ..
                        } if candidate == token
                    )
            ) || matches!(
                frame,
                super::frame::EvalFrame::Handler(frame)
                    if matches!(
                        &*frame.inner,
                        crate::control::Continuation::HandlerDispatch {
                            deferred: Some(candidate), ..
                        } if *candidate == token
                    )
            )
        });
        let boundary =
            boundary.expect("deferred continuation transfer must retain its handler boundary");
        let retained = match &mut self.stack[boundary] {
            // Resume the captured body under its original lexical Handler.
            // Only the arm's dispatch is replaced; the boundary still closes
            // the dynamic scope when the resumed body returns.
            super::frame::EvalFrame::Handler(frame) => {
                frame.inner = crate::control::Continuation::BlockValue.into();
                boundary + 1
            }
            super::frame::EvalFrame::Continuation(_) => boundary,
            _ => unreachable!("selected deferred transfer has a dispatch boundary"),
        };
        // A source scheduler method can transfer from inside another checked
        // program. The dispatch continuation belongs to the program outside
        // the first discarded call boundary.
        if let Some(call) = self.stack[retained..].iter().find_map(|frame| match frame {
            EvalFrame::ProgramReturn(call) => Some(call),
            _ => None,
        }) {
            self.program_scope = call.parent.clone();
            local.model_policy = call.parent_model_policy.clone();
            local.agent_context = call.parent_agent_context.clone();
        }
        self.truncate_to(retained);
        self.resume(signal);
    }

    pub(crate) fn resume_model_result(
        &mut self,
        result: Result<etas_host::ModelResponse, etas_host::HostError>,
    ) {
        assert!(
            self.input
                .replace(MachineInput::ModelResult(result))
                .is_none(),
            "EvalMachine cannot resume while another input is pending"
        );
    }

    pub(crate) fn resume_tool_result(
        &mut self,
        result: Result<etas_host::ToolResponse, etas_host::HostError>,
    ) {
        assert!(
            self.input
                .replace(MachineInput::ToolResult(result))
                .is_none(),
            "EvalMachine cannot resume while another input is pending"
        );
    }

    pub(crate) fn resume_source_tool_approved(&mut self) {
        assert!(
            self.input
                .replace(MachineInput::SourceToolApproved)
                .is_none(),
            "EvalMachine cannot resume while another input is pending"
        );
    }

    pub(crate) fn retry_boundary_failure(
        &mut self,
        ctx: &mut EvalContext<'_>,
        boundary_continuation: crate::control::Continuation,
        span: Span,
        message: String,
    ) -> Option<ControlSignal> {
        // The dynamic retry stack belongs to this task across checked programs.
        // Defer execution until the machine has unwound to the owning program.
        (!ctx.task.retry_stack.is_empty())
            .then(|| retry_failure(boundary_continuation, span, message))
    }

    pub(super) fn propagate_retry_failure(
        &mut self,
        ctx: &mut EvalContext<'_>,
        continuation: Continuation,
        span: Span,
        message: String,
    ) -> Result<ControlSignal, MachinePoll> {
        match continuation {
            attempt @ Continuation::RetryAttempt { .. } => {
                return Ok(ctx
                    .retry_boundary_failure_signal(attempt, span, &message)
                    .expect("retry attempt handles its boundary failure"));
            }
            Continuation::CallBoundary { outer, .. }
            | Continuation::ScopedModelPolicy { inner: outer, .. } => {
                return Ok(retry_failure(outer.into_value(), span, message));
            }
            Continuation::HandlerDispatch { outer, deferred } => {
                if let Some(token) = deferred {
                    ctx.scheduler
                        .dispose_continuation(token, ctx.current_task)
                        .map_err(|error| {
                            MachinePoll::Fault(crate::control::ExecutionFault::new(
                                etas_core::AnalysisDiagnosticCode::UnhandledRuntimeError,
                                span,
                                error.to_string(),
                            ))
                        })?;
                }
                return Ok(retry_failure(outer.into_value(), span, message));
            }
            Continuation::HandleBoundary {
                scope_id,
                inner,
                handlers,
                span: boundary_span,
                frame,
            } => {
                if !matches!(*inner, Continuation::BlockValue) {
                    self.push_continuation_frames(Continuation::HandleBoundary {
                        scope_id,
                        inner: Continuation::BlockValue.into(),
                        handlers,
                        span: boundary_span,
                        frame,
                    });
                    return Ok(retry_failure(inner.into_value(), span, message));
                }
                ctx.close_handler_scope(scope_id, boundary_span)
                    .map_err(MachinePoll::Fault)?;
            }
            Continuation::RestoreModelPolicy { previous, inner } => {
                if !matches!(*inner, Continuation::BlockValue) {
                    self.push_continuation_frames(Continuation::RestoreModelPolicy {
                        previous,
                        inner: Continuation::BlockValue.into(),
                    });
                    return Ok(retry_failure(inner.into_value(), span, message));
                }
                ctx.task.model_policy = *previous;
            }
            Continuation::ReenterHandler { boundary } => {
                // Forwarding already closed this scope. Abandon its saved body
                // without reopening the handler or closing it a second time.
                let Continuation::HandleBoundary { inner, .. } = boundary.into_value() else {
                    unreachable!("handler reentry owns its saved handler boundary")
                };
                return Ok(retry_failure(inner.into_value(), span, message));
            }
            Continuation::ResidualScope(scope) => {
                ctx.exit_residual_scope(
                    &scope,
                    crate::control::residual::ResidualExitKind::ErrorPropagation,
                    span,
                )
                .map_err(MachinePoll::Fault)?;
            }
            Continuation::AgentExecutionState(previous) => {
                ctx.restore_agent_context(*previous);
            }
            _ => {}
        }

        let Some(frame) = self.pop_frame() else {
            return Err(MachinePoll::Fault(crate::control::ExecutionFault::new(
                etas_core::AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                message,
            )));
        };
        let continuation = match frame {
            EvalFrame::ProgramReturn(frame) => {
                return Err(self.unwind_program_retry_failure(ctx, *frame, message));
            }
            EvalFrame::PureBoundary(_) => Continuation::BlockValue,
            frame => frame
                .into_continuation()
                .expect("program-local retry frame"),
        };
        Ok(retry_failure(continuation, span, message))
    }
}

fn retry_failure(continuation: Continuation, span: Span, message: String) -> ControlSignal {
    ControlSignal::pending_continuation(PendingContinuation {
        continuation,
        input: ContinuationInput::RetryFailure { span, message },
    })
}

#[cfg(test)]
mod tests;

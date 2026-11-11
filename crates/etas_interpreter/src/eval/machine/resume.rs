use etas_core::Span;

use crate::{control::ControlSignal, eval::EvalContext};

use super::state::{EvalMachine, MachineInput};

impl EvalMachine {
    pub(crate) fn resume(&mut self, signal: ControlSignal) {
        assert!(
            self.input.replace(MachineInput::Signal(signal)).is_none(),
            "EvalMachine cannot resume while another signal is pending"
        );
    }

    pub(crate) fn transfer_from(
        &mut self,
        token: crate::value::ContinuationTokenValue,
        continuation: crate::control::Continuation,
        value: crate::value::InterpValue,
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
        self.truncate_to(retained);
        self.resume(ControlSignal::pending_continuation(
            crate::control::PendingContinuation {
                continuation,
                input: crate::control::ContinuationInput::Value(value),
            },
        ));
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
        if let Some(signal) =
            ctx.retry_boundary_failure_signal(boundary_continuation, span, &message)
        {
            return Some(signal);
        }

        for index in (0..self.frames().len()).rev() {
            if matches!(
                self.frames()[index],
                super::frame::EvalFrame::ProgramReturn(_)
            ) {
                return Some(ControlSignal::missing_checked_fact(
                    "cross-revision retry requires a checked continuation transition",
                    span,
                ));
            }
            let Some(continuation) = self.frames()[index].retry_continuation() else {
                continue;
            };
            if let Some(signal) = ctx.retry_boundary_failure_signal(continuation, span, &message) {
                for frame in self.frames()[index..].iter().rev() {
                    if let super::frame::EvalFrame::Continuation(frame) = frame {
                        if let crate::control::Continuation::AgentExecutionState(previous) =
                            &frame.continuation
                        {
                            ctx.restore_agent_context((**previous).clone());
                        }
                    }
                }
                self.truncate_to(index);
                return Some(signal);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests;

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
            ctx.retry_boundary_failure_signal(boundary_continuation, span, message.clone())
        {
            return Some(signal);
        }

        for index in (0..self.frames().len()).rev() {
            let continuation = self.frames()[index].continuation_clone();
            if let Some(signal) =
                ctx.retry_boundary_failure_signal(continuation, span, message.clone())
            {
                self.truncate_to(index);
                return Some(signal);
            }
        }
        None
    }
}

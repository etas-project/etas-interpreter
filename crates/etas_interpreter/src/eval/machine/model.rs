use super::{
    frame::{EvalFrame, ModelRequestFrame},
    state::{EvalMachine, MachinePoll, PendingBoundary},
};
use crate::{
    control::{
        CallPurity, Continuation, ControlSignal, ExecutionFault, ModelDecode, PendingCall,
        PendingModel,
    },
    eval::EvalContext,
    value::InterpValue,
};
use etas_core::{AnalysisDiagnosticCode, Span};
use etas_host::{HostError, ModelOutput, ModelResponse, ModelUsage};

#[derive(Clone, Debug)]
pub struct ModelStepProviderData {
    frame: Box<ModelRequestFrame>,
    output: ModelOutput,
    usage: Option<ModelUsage>,
}

type Transition = Result<ControlSignal, MachinePoll>;

impl EvalMachine {
    pub(super) fn begin_model(
        &mut self,
        ctx: &mut EvalContext<'_>,
        mut pending: PendingModel,
    ) -> Transition {
        let outer = std::mem::replace(&mut pending.continuation, Continuation::BlockValue);
        self.push_continuation_frames(outer);
        let key = ctx
            .model_boundary_key(&pending)
            .map_err(MachinePoll::Fault)?;
        if let Some(value) = ctx
            .completed_host_boundary_result(
                &crate::orchestration::BoundaryOccurrenceId::HostRequest(pending.request.id),
                "model",
                &key,
                pending.span,
            )
            .map_err(MachinePoll::Fault)?
        {
            return Ok(ControlSignal::Value(value));
        }
        self.push_frame(EvalFrame::ModelRequest(Box::new(ModelRequestFrame {
            boundary_request_id: pending.request.id,
            pending: pending.clone(),
            boundary_key: key,
            outer_continuation: Continuation::BlockValue,
        })));
        Err(MachinePoll::Yield(PendingBoundary::Model(Box::new(
            pending,
        ))))
    }

    pub(super) fn resume_model_host_result(
        &mut self,
        ctx: &mut EvalContext<'_>,
        result: Result<ModelResponse, HostError>,
    ) -> Transition {
        let Some(EvalFrame::ModelRequest(frame)) = self.pop_frame() else {
            return Err(machine_fault(
                "model response has no active request frame",
                ctx,
            ));
        };
        let span = frame.pending.span;
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                let message = format!(
                    "model host boundary failed: {}: {}",
                    error.code.as_str(),
                    error.message
                );
                return match self.retry_boundary_failure(
                    ctx,
                    frame.outer_continuation,
                    span,
                    message.clone(),
                ) {
                    Some(signal) => Ok(signal),
                    None => Err(MachinePoll::Fault(ExecutionFault::new(
                        AnalysisDiagnosticCode::UnhandledRuntimeError,
                        span,
                        message,
                    ))),
                };
            }
        };
        let output = response.result.map_err(|error| {
            MachinePoll::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                format!(
                    "model generation failed: {}: {}",
                    error.code.as_str(),
                    error.message
                ),
            ))
        })?;
        let value = match frame.pending.decode {
            ModelDecode::Step { result, .. } => {
                if let Some((target, entries)) = ctx
                    .step_provider_data_builder(result, &output.message.provider_data)
                    .map_err(|error| {
                        MachinePoll::Fault(ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            error,
                        ))
                    })?
                {
                    return Ok(ControlSignal::pending_call(PendingCall {
                        purity: CallPurity::Pure,
                        site: None,
                        target,
                        args: vec![entries],
                        span,
                        continuation: Continuation::ModelStepProviderData(Box::new(
                            ModelStepProviderData {
                                frame,
                                output,
                                usage: response.usage,
                            },
                        )),
                    }));
                }
                ctx.step_result_value(&frame.pending, output, response.usage, None)
                    .map_err(MachinePoll::Fault)?
            }
            ModelDecode::ModelResponse => ctx
                .try_model_result_value(&frame.pending, output, response.id, response.usage)
                .map_err(|error| {
                    MachinePoll::Fault(ExecutionFault::new(error.code, span, error.message))
                })?,
            ModelDecode::String | ModelDecode::Typed(_) => {
                return Err(MachinePoll::Fault(ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    "full inference decoding belongs to the checked Agent Step controller",
                )));
            }
        };
        ctx.complete_model_boundary(*frame, value)
            .map_err(MachinePoll::Fault)
    }
}

impl EvalContext<'_> {
    pub(crate) fn complete_model_step_provider_data(
        &mut self,
        state: ModelStepProviderData,
        provider_data: InterpValue,
    ) -> ControlSignal {
        let result = self
            .step_result_value(
                &state.frame.pending,
                state.output,
                state.usage,
                Some(provider_data),
            )
            .and_then(|value| self.complete_model_boundary(*state.frame, value));
        result.unwrap_or_else(|fault| ControlSignal::Fault(Box::new(fault)))
    }

    fn complete_model_boundary(
        &mut self,
        frame: ModelRequestFrame,
        value: InterpValue,
    ) -> Result<ControlSignal, ExecutionFault> {
        self.record_completed_host_boundary(
            crate::orchestration::BoundaryOccurrenceId::HostRequest(frame.boundary_request_id),
            "model",
            frame.boundary_key,
            value.clone(),
            frame.pending.span,
        )?;
        Ok(self.apply_continuation(frame.outer_continuation, value))
    }
}

fn machine_fault(message: &str, ctx: &EvalContext<'_>) -> MachinePoll {
    let span: Span = crate::diagnostics::item_span(ctx.checked, ctx.entry_item);
    MachinePoll::Fault(ExecutionFault::new(
        AnalysisDiagnosticCode::MissingCheckedFact,
        span,
        message,
    ))
}

use super::*;
use crate::{
    control::{
        Continuation, ContinuationInput, ControlSignal, ExecutionFault, PendingContinuation,
    },
    eval::{EvalContext, host_value::decode_checked_host_value},
    intrinsic::dispatch::CheckedStdIntrinsicCall,
    orchestration::BoundaryOccurrenceId,
    value::conversation::protocol,
};
use etas_core::{AnalysisDiagnosticCode, Span};
use etas_host::{HostTraceFieldSensitivity as Sensitivity, HostTracePayload, HostValue};

impl EvalContext<'_> {
    pub(in crate::eval) fn tool_invocation_error(
        &self,
        invocation: &ToolInvocation,
        error: ToolInvocationError,
    ) -> ControlSignal {
        match error {
            ToolInvocationError::Rejected(error) => {
                self.tool_invocation_rejection(invocation, error)
            }
            ToolInvocationError::Fault(fault) => ControlSignal::Fault(Box::new(fault)),
        }
    }
    pub(in crate::eval) fn execute_tool_invoke(
        &mut self,
        call: &CheckedStdIntrinsicCall,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        self.execute_checked_tool_invoke(
            crate::control::CheckedAgentOperation::standard(call),
            args,
            span,
        )
    }

    pub(in crate::eval) fn execute_checked_tool_invoke(
        &mut self,
        call: crate::control::CheckedAgentOperation,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let Some(context) = self.task.agent_context.clone() else {
            return ControlSignal::missing_checked_fact(
                "invoke requires an active checked agent context",
                span,
            );
        };
        let [input]: [InterpValue; 1] = match args.try_into() {
            Ok(args) => args,
            Err(_) => {
                return ControlSignal::missing_checked_fact(
                    "invoke requires its checked single ToolCall argument",
                    span,
                );
            }
        };
        let invocation = Box::new(ToolInvocation {
            call,
            input,
            context,
            request: self.next_host_request_id(),
            span,
        });
        ControlSignal::pending_continuation(PendingContinuation {
            continuation: Continuation::ToolInvocationStart(invocation),
            input: ContinuationInput::Value(InterpValue::Unit),
        })
    }

    pub(in crate::eval) fn tool_invocation_rejection(
        &self,
        invocation: &ToolInvocation,
        rejection: ToolRejection,
    ) -> ControlSignal {
        let call_id = match protocol::field(&invocation.input, "id") {
            Ok(InterpValue::String(id)) => id.to_string(),
            _ => return ControlSignal::invalid_arguments(rejection.message, invocation.span),
        };
        let error = HostValue::Record(vec![
            ("code".into(), HostValue::String(rejection.code.into())),
            ("message".into(), HostValue::String(rejection.message)),
            (
                "path".into(),
                HostValue::Variant {
                    name: "None".into(),
                    fields: Vec::new(),
                },
            ),
        ]);
        self.tool_feedback(invocation, call_id, "Err", error)
    }

    pub(in crate::eval) fn tool_feedback(
        &self,
        invocation: &ToolInvocation,
        call_id: String,
        variant: &str,
        payload: HostValue,
    ) -> ControlSignal {
        let value = HostValue::Record(vec![
            ("call_id".into(), HostValue::String(call_id)),
            (
                "result".into(),
                HostValue::Variant {
                    name: variant.into(),
                    fields: vec![payload],
                },
            ),
        ]);
        match decode_checked_host_value(value, invocation.call.result_type, self.checked) {
            Ok(value) => ControlSignal::Value(value),
            Err(reason) => ControlSignal::missing_checked_fact(reason, invocation.span),
        }
    }

    pub(in crate::eval) fn tool_invocation_occurrence(
        &self,
        invocation: &ToolInvocation,
        resolved: &ResolvedToolInvocation,
    ) -> BoundaryOccurrenceId {
        BoundaryOccurrenceId::SourceToolCall {
            model_request: invocation.context.request,
            call_id: resolved.call_id.clone(),
        }
    }

    pub(in crate::eval) fn tool_invocation_fingerprint(
        &self,
        invocation: &ToolInvocation,
        resolved: &ResolvedToolInvocation,
    ) -> Result<etas_host::replay::ReplayFingerprint, ExecutionFault> {
        let payload = HostTracePayload::new("native_tool", &resolved.identity)
            .with_field(
                "revision",
                HostValue::String(resolved.revision.clone()),
                Sensitivity::Public,
            )
            .with_field(
                "context_request",
                HostValue::UInt(invocation.context.request.0.into()),
                Sensitivity::Public,
            )
            .with_field(
                "call_id",
                HostValue::String(resolved.call_id.clone()),
                Sensitivity::Public,
            )
            .with_field("args", resolved.args.clone(), Sensitivity::Sensitive);
        self.replay_payload_fingerprint(&payload, invocation.span)
    }

    pub(in crate::eval) fn complete_tool_invocation(
        &mut self,
        invocation: ToolInvocation,
        value: InterpValue,
    ) -> ControlSignal {
        let resolved = match self.resolve_tool_invocation(&invocation) {
            Ok(resolved) => resolved,
            Err(error) => return self.tool_invocation_error(&invocation, error),
        };
        if let Err(message) = crate::value::contract::runtime::validate_value(
            self.checked,
            self.plan,
            &value,
            resolved.binding.output,
            &self.storage_limits,
        ) {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::InvalidArguments,
                invocation.span,
                message,
            );
        }
        let result = crate::eval::interp_to_host_value(&value).and_then(|value| {
            self.storage_limits
                .result_value_size(&value)
                .map_err(|error| error.message)?;
            if let Some(schema) = &resolved.binding.schema.output {
                schema::validate_host_value_schema(&value, schema, "result")?;
            }
            Ok(value)
        });
        let payload = match result {
            Ok(value) => value,
            Err(message) => {
                return ControlSignal::fault(
                    AnalysisDiagnosticCode::InvalidArguments,
                    invocation.span,
                    message,
                );
            }
        };
        let key = match self.tool_invocation_fingerprint(&invocation, &resolved) {
            Ok(key) => key,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let feedback = self.tool_feedback(&invocation, resolved.call_id.clone(), "Ok", payload);
        let ControlSignal::Value(value) = feedback else {
            return feedback;
        };
        let occurrence = self.tool_invocation_occurrence(&invocation, &resolved);
        match self.record_completed_host_boundary(
            occurrence,
            "tool",
            key,
            value.clone(),
            invocation.span,
        ) {
            Ok(()) => ControlSignal::Value(value),
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        }
    }
}

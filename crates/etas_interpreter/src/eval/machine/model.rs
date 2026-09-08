use etas_core::{AnalysisDiagnosticCode, Span};
use etas_host::{
    HostError, HostJsonValue, HostSchema, HostValue, ModelContent, ModelMessage, ModelResponse,
    ModelRole, ModelToolCall, ModelToolChoice, PolicySubject, ToolRef, ToolRequest, ToolResponse,
};

use crate::{
    control::{Continuation, ControlSignal, ExecutionFault, PendingModel, SourceToolBinding},
    eval::{EvalContext, interp_to_host_value},
    orchestration::WorkflowEvent,
};

use super::{
    frame::{EvalFrame, HostToolProgress, ModelLoopFrame, ModelRepairState, SourceToolReturnFrame},
    model_repair::{ModelRepairExhausted, ModelRepairKind, ModelRepairPolicy},
    state::{EvalMachine, MachinePoll, PendingBoundary, PendingTool, PendingToolDispatch},
};

type Transition = Result<ControlSignal, MachinePoll>;

impl EvalMachine {
    pub(super) fn begin_model(
        &mut self,
        ctx: &mut EvalContext<'_>,
        mut pending: PendingModel,
    ) -> Transition {
        let outer_continuation =
            std::mem::replace(&mut pending.continuation, Continuation::BlockValue);
        if let Some(value) = ctx.replayed_model_result(&pending) {
            return Ok(ctx.apply_continuation(outer_continuation, value));
        }
        let frame = ModelLoopFrame {
            boundary_key: ctx.model_boundary_key(&pending),
            pending,
            round: 0,
            repair: ModelRepairState::default(),
            last_tool_error: None,
            remaining_tool_calls: Vec::new(),
            completed_tool_result: false,
            current_host_tool: None,
            outer_continuation,
        };
        self.yield_model(frame)
    }

    pub(super) fn resume_model_host_result(
        &mut self,
        ctx: &mut EvalContext<'_>,
        result: Result<ModelResponse, HostError>,
    ) -> Transition {
        let frame = self.pop_model_loop_frame()?;
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                return self.model_boundary_failure(
                    ctx,
                    frame,
                    format!("model host boundary failed: {}", format_host_error(&error)),
                );
            }
        };
        self.handle_model_response(ctx, frame, response)
    }

    pub(super) fn resume_tool_host_result(
        &mut self,
        ctx: &mut EvalContext<'_>,
        result: Result<ToolResponse, HostError>,
    ) -> Transition {
        let mut frame = self.pop_model_loop_frame()?;
        let progress = frame.current_host_tool.take().ok_or_else(|| {
            machine_abort(
                "tool host result has no active model tool-call frame",
                frame.pending.span,
            )
        })?;
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                return self.model_boundary_failure(
                    ctx,
                    frame,
                    format!("tool host boundary failed: {}", format_host_error(&error)),
                );
            }
        };
        let tool_value = match response.result {
            Ok(value) => value,
            Err(error) => {
                return self.model_boundary_failure(
                    ctx,
                    frame,
                    format!("tool `{}` failed: {}", progress.call.tool, error.message),
                );
            }
        };
        ctx.record_completed_host_value_boundary(
            crate::orchestration::BoundaryOccurrenceId::HostRequest(response.id),
            "tool",
            progress.boundary_key,
            tool_value.clone(),
        );
        append_tool_result(&mut frame, progress.call.id, tool_value);
        self.advance_tool_calls(ctx, frame)
    }

    pub(super) fn resume_source_tool_approved_input(
        &mut self,
        ctx: &mut EvalContext<'_>,
    ) -> Transition {
        let frame = self.pop_source_tool_return_frame()?;
        let span = frame.model_loop.pending.span;
        let (target, args) = ctx
            .prepare_source_tool_call(frame.binding.item, frame.args.clone(), span)
            .map_err(MachinePoll::Fault)?;
        self.push_frame(EvalFrame::SourceToolReturn(frame));
        Ok(ctx.execute_call_target(target, args, span))
    }

    pub(super) fn resume_source_tool_value(
        &mut self,
        ctx: &mut EvalContext<'_>,
        frame: SourceToolReturnFrame,
        value: crate::value::InterpValue,
    ) -> Transition {
        let host_value = interp_to_host_value(&value).map_err(|error| {
            machine_abort(
                format!(
                    "source tool `{}` returned a value that cannot be encoded for the model tool protocol: {error}",
                    frame.tool_name,
                ),
                frame.model_loop.pending.span,
            )
        })?;
        if let Some(schema) = &frame.output_schema {
            validate_host_value_schema(&host_value, schema, "result").map_err(|message| {
                machine_abort(
                    format!(
                        "source tool `{}` returned invalid output: {message}",
                        frame.tool_name
                    ),
                    frame.model_loop.pending.span,
                )
            })?;
        }
        let occurrence = crate::orchestration::BoundaryOccurrenceId::SourceToolCall {
            model_request: frame.model_loop.pending.request.id,
            call_id: frame.tool_call_id.clone(),
        };
        ctx.record_completed_host_boundary(occurrence, "tool", frame.boundary_key, value);
        let mut model_loop = *frame.model_loop;
        append_tool_result(&mut model_loop, frame.tool_call_id, host_value);
        self.advance_tool_calls(ctx, model_loop)
    }

    fn handle_model_response(
        &mut self,
        ctx: &mut EvalContext<'_>,
        mut frame: ModelLoopFrame,
        response: ModelResponse,
    ) -> Transition {
        if response.tool_calls.is_empty()
            || matches!(
                frame.pending.decode,
                crate::control::ModelDecode::ModelResponse
            )
        {
            if response.tool_calls.is_empty()
                && !matches!(frame.pending.request.tool_choice, ModelToolChoice::Auto)
                && !matches!(
                    frame.pending.decode,
                    crate::control::ModelDecode::ModelResponse
                )
            {
                let repair = ModelRepairPolicy::new(frame.pending.max_tool_rounds)
                    .required_tool_choice(frame.round, &frame.pending.request.tool_choice)
                    .map_err(|exhausted| {
                        record_model_repair_exhausted(ctx, frame.pending.span, exhausted)
                    })?;
                record_model_repair_attempt(ctx, &repair);
                frame.round = repair.attempt;
                frame.repair.attempts = repair.attempt;
                frame.repair.last_kind = Some(repair.kind.as_str().to_owned());
                frame.last_tool_error = Some(repair.reason.clone());
                frame.pending.request.messages.push(response.message);
                frame.pending.request.messages.push(repair.message);
                frame.pending.request.id = ctx.next_host_request_id();
                return self.yield_model(frame);
            }

            let response_for_repair = response.clone();
            match ctx.try_model_result_value(&frame.pending, response) {
                Ok(value) => {
                    ctx.record_completed_host_boundary(
                        crate::orchestration::BoundaryOccurrenceId::HostRequest(
                            frame.pending.request.id,
                        ),
                        "model",
                        frame.boundary_key,
                        value.clone(),
                    );
                    return Ok(ctx.apply_continuation(frame.outer_continuation, value));
                }
                Err(error) if error.retryable_typed_output => {
                    let repair = ModelRepairPolicy::new(frame.pending.max_tool_rounds)
                        .typed_output(frame.round, &error.message)
                        .map_err(|exhausted| {
                            record_model_repair_exhausted(ctx, frame.pending.span, exhausted)
                        })?;
                    record_model_repair_attempt(ctx, &repair);
                    frame.round = repair.attempt;
                    frame.repair.attempts = repair.attempt;
                    frame.repair.last_kind = Some(repair.kind.as_str().to_owned());
                    frame
                        .pending
                        .request
                        .messages
                        .push(response_for_repair.message);
                    frame.pending.request.messages.push(repair.message);
                    frame.pending.request.id = ctx.next_host_request_id();
                    return self.yield_model(frame);
                }
                Err(error) => {
                    let message = error.message.clone();
                    ctx.record_model_result_error(&frame.pending, error);
                    return Err(machine_abort(message, frame.pending.span));
                }
            }
        }

        if frame.round >= frame.pending.max_tool_rounds {
            let detail = frame
                .last_tool_error
                .as_deref()
                .map(|error| format!("; last tool-loop error: {error}"))
                .unwrap_or_default();
            return Err(machine_abort(
                format!("model tool-call loop exceeded the maximum number of rounds{detail}"),
                frame.pending.span,
            ));
        }
        frame.round += 1;
        let mut assistant = response.message;
        if assistant.tool_calls.is_empty() {
            assistant.tool_calls = response.tool_calls.clone();
        }
        frame.pending.request.messages.push(assistant);
        frame.remaining_tool_calls = response.tool_calls;
        frame.completed_tool_result = false;
        self.advance_tool_calls(ctx, frame)
    }

    fn advance_tool_calls(
        &mut self,
        ctx: &mut EvalContext<'_>,
        mut frame: ModelLoopFrame,
    ) -> Transition {
        while !frame.remaining_tool_calls.is_empty() {
            let call = frame.remaining_tool_calls.remove(0);
            if let ModelToolChoice::RequiredTool(required) = &frame.pending.request.tool_choice
                && call.tool != *required
            {
                return Err(machine_abort(
                    format!(
                        "model called tool `{}` but required tool `{required}` was requested",
                        call.tool
                    ),
                    frame.pending.span,
                ));
            }
            let Some(schema) = frame
                .pending
                .request
                .tools
                .iter()
                .find(|schema| schema.tool.name == call.tool)
                .cloned()
            else {
                let available = frame
                    .pending
                    .request
                    .tools
                    .iter()
                    .map(|schema| schema.tool.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(machine_abort(
                    format!(
                        "model requested unavailable tool `{}`; available tools: [{available}]",
                        call.tool
                    ),
                    frame.pending.span,
                ));
            };
            if let Err(message) = validate_tool_args(&call, &schema.input) {
                frame.last_tool_error = Some(message.clone());
                frame
                    .pending
                    .request
                    .messages
                    .push(invalid_tool_args_message(&call.id, &call.tool, message));
                continue;
            }

            if let Some(binding) = frame
                .pending
                .source_tools
                .iter()
                .find(|binding| binding.name == call.tool)
                .cloned()
            {
                let key = source_tool_boundary_key(&binding, &call.args);
                let occurrence = crate::orchestration::BoundaryOccurrenceId::SourceToolCall {
                    model_request: frame.pending.request.id,
                    call_id: call.id.clone(),
                };
                if let Some(replayed) =
                    ctx.completed_host_boundary_result(&occurrence, "tool", &key)
                {
                    let value = interp_to_host_value(&replayed).map_err(|error| {
                        machine_abort(
                            format!(
                                "completed source tool boundary `{}` cannot be encoded for the model protocol: {error}",
                                call.tool,
                            ),
                            frame.pending.span,
                        )
                    })?;
                    append_tool_result(&mut frame, call.id, value);
                    continue;
                }
                let source_return = SourceToolReturnFrame {
                    tool_call_id: call.id,
                    tool_name: call.tool,
                    binding: binding.clone(),
                    args: call.args.clone(),
                    boundary_key: key,
                    output_schema: schema.output,
                    model_loop: Box::new(frame),
                };
                let pending = PendingTool {
                    policy_ref: source_return.model_loop.pending.request.policy_ref.clone(),
                    policy_subject: source_tool_policy_subject(&binding),
                    span: source_return.model_loop.pending.span,
                    dispatch: PendingToolDispatch::Source,
                };
                self.push_frame(EvalFrame::SourceToolReturn(source_return));
                return Err(MachinePoll::Yield(PendingBoundary::Tool(Box::new(pending))));
            }

            let request = ToolRequest {
                id: ctx.next_host_request_id(),
                tool: schema.tool,
                args: call.args.clone(),
                authority: frame.pending.request.authority.clone(),
                trace: frame.pending.request.trace.clone(),
                budget: frame.pending.request.budget.clone(),
            };
            let key = tool_boundary_key(&request);
            if let Some(value) = ctx.completed_host_boundary_host_result(
                &crate::orchestration::BoundaryOccurrenceId::HostRequest(request.id),
                "tool",
                &key,
            ) {
                append_tool_result(&mut frame, call.id, value);
                continue;
            }
            frame.current_host_tool = Some(HostToolProgress {
                call,
                boundary_key: key,
            });
            let pending = PendingTool {
                policy_ref: frame.pending.request.policy_ref.clone(),
                policy_subject: tool_policy_subject(&request),
                span: frame.pending.span,
                dispatch: PendingToolDispatch::Host(Box::new(request)),
            };
            self.push_frame(EvalFrame::ModelLoop(Box::new(frame)));
            return Err(MachinePoll::Yield(PendingBoundary::Tool(Box::new(pending))));
        }

        if frame.completed_tool_result {
            if !matches!(frame.pending.request.tool_choice, ModelToolChoice::Auto) {
                frame.pending.request.tools.clear();
                frame.pending.source_tools.clear();
            }
            frame.pending.request.tool_choice = ModelToolChoice::Auto;
        }
        frame.pending.request.id = ctx.next_host_request_id();
        self.yield_model(frame)
    }

    fn yield_model(&mut self, frame: ModelLoopFrame) -> Transition {
        let pending = frame.pending.clone();
        self.push_frame(EvalFrame::ModelLoop(Box::new(frame)));
        Err(MachinePoll::Yield(PendingBoundary::Model(Box::new(
            pending,
        ))))
    }

    fn pop_model_loop_frame(&mut self) -> Result<ModelLoopFrame, MachinePoll> {
        match self.pop_frame() {
            Some(EvalFrame::ModelLoop(frame)) => Ok(*frame),
            Some(frame) => {
                let span = frame_span(&frame);
                self.push_frame(frame);
                Err(machine_abort(
                    "model/tool host response does not match the active machine frame",
                    span,
                ))
            }
            None => Err(machine_abort(
                "model/tool host response reached an empty machine stack",
                Span::empty(etas_core::SourceId(0), etas_core::TextSize(0)),
            )),
        }
    }

    fn pop_source_tool_return_frame(&mut self) -> Result<SourceToolReturnFrame, MachinePoll> {
        match self.pop_frame() {
            Some(EvalFrame::SourceToolReturn(frame)) => Ok(frame),
            Some(frame) => {
                let span = frame_span(&frame);
                self.push_frame(frame);
                Err(machine_abort(
                    "source tool approval does not match the active machine frame",
                    span,
                ))
            }
            None => Err(machine_abort(
                "source tool approval reached an empty machine stack",
                Span::empty(etas_core::SourceId(0), etas_core::TextSize(0)),
            )),
        }
    }

    fn model_boundary_failure(
        &mut self,
        ctx: &mut EvalContext<'_>,
        frame: ModelLoopFrame,
        message: String,
    ) -> Transition {
        if let Some(signal) = self.retry_boundary_failure(
            ctx,
            frame.outer_continuation,
            frame.pending.span,
            message.clone(),
        ) {
            Ok(signal)
        } else {
            Err(machine_abort(message, frame.pending.span))
        }
    }
}

fn append_tool_result(frame: &mut ModelLoopFrame, id: String, value: HostValue) {
    frame.pending.request.messages.push(ModelMessage {
        role: ModelRole::Tool,
        content: vec![ModelContent::Value(value)],
        tool_call_id: Some(id),
        tool_calls: Vec::new(),
    });
    frame.completed_tool_result = true;
}

fn invalid_tool_args_message(id: &str, tool: &str, message: String) -> ModelMessage {
    ModelMessage {
        role: ModelRole::Tool,
        content: vec![ModelContent::Value(HostValue::Record(vec![
            (
                "kind".to_owned(),
                HostValue::String("InvalidToolArguments".to_owned()),
            ),
            ("tool".to_owned(), HostValue::String(tool.to_owned())),
            ("message".to_owned(), HostValue::String(message)),
            ("retryable".to_owned(), HostValue::Bool(true)),
        ]))],
        tool_call_id: Some(id.to_owned()),
        tool_calls: Vec::new(),
    }
}

fn validate_tool_args(call: &ModelToolCall, schema: &HostSchema) -> Result<(), String> {
    validate_host_value_schema(&call.args, schema, "args").map_err(|message| {
        format!(
            "model supplied invalid arguments for tool `{}`: {message}",
            call.tool
        )
    })
}

fn record_model_repair_attempt(
    ctx: &mut EvalContext<'_>,
    repair: &super::model_repair::ModelRepairDirective,
) {
    ctx.events.push(WorkflowEvent::ModelRepairAttempted {
        kind: repair.kind.as_str().to_owned(),
        attempt: repair.attempt,
        reason: repair.reason.clone(),
    });
}

fn record_model_repair_exhausted(
    ctx: &mut EvalContext<'_>,
    span: Span,
    exhausted: ModelRepairExhausted,
) -> MachinePoll {
    ctx.events.push(WorkflowEvent::ModelRepairExhausted {
        kind: exhausted.kind.as_str().to_owned(),
        attempts: exhausted.attempts,
        reason: exhausted.reason.clone(),
    });
    let code = match exhausted.kind {
        ModelRepairKind::RequiredToolChoice => AnalysisDiagnosticCode::UnhandledRuntimeError,
        ModelRepairKind::TypedOutput => AnalysisDiagnosticCode::InvalidArguments,
    };
    MachinePoll::Fault(ExecutionFault::new(
        code,
        span,
        format!(
            "model repair exhausted: kind={}, attempts={}, reason={}",
            exhausted.kind.as_str(),
            exhausted.attempts,
            exhausted.reason
        ),
    ))
}

fn tool_policy_subject(request: &ToolRequest) -> PolicySubject {
    let identity = tool_identity(&request.tool);
    let mut attributes = vec![
        (
            "action_kind".to_owned(),
            HostValue::String("tool".to_owned()),
        ),
        (
            "qualified_action".to_owned(),
            HostValue::String("Tool.call".to_owned()),
        ),
        (
            "tool".to_owned(),
            HostValue::String(request.tool.name.clone()),
        ),
        (
            "qualified_tool".to_owned(),
            HostValue::String(identity.to_owned()),
        ),
        (
            "resource".to_owned(),
            HostValue::String(identity.to_owned()),
        ),
    ];
    if let Some(symbol) = request.tool.std_symbol {
        attributes.push((
            "std_symbol".to_owned(),
            HostValue::String(format!("{symbol:?}")),
        ));
    }
    PolicySubject {
        kind: "tool".to_owned(),
        attributes,
    }
}

fn source_tool_policy_subject(binding: &SourceToolBinding) -> PolicySubject {
    let identity = source_tool_identity(binding);
    PolicySubject {
        kind: "tool".to_owned(),
        attributes: vec![
            (
                "action_kind".to_owned(),
                HostValue::String("tool".to_owned()),
            ),
            (
                "qualified_action".to_owned(),
                HostValue::String("Tool.call".to_owned()),
            ),
            ("tool".to_owned(), HostValue::String(binding.name.clone())),
            (
                "qualified_tool".to_owned(),
                HostValue::String(identity.to_owned()),
            ),
            (
                "resource".to_owned(),
                HostValue::String(identity.to_owned()),
            ),
            (
                "source_item".to_owned(),
                HostValue::String(format!("{:?}", binding.item)),
            ),
        ],
    }
}

fn tool_boundary_key(request: &ToolRequest) -> String {
    format!(
        "tool:{}:std={:?}:args={:?}",
        tool_identity(&request.tool),
        request.tool.std_symbol,
        request.args
    )
}

fn source_tool_boundary_key(binding: &SourceToolBinding, args: &HostValue) -> String {
    format!(
        "tool:{}:source_item={:?}:args={args:?}",
        source_tool_identity(binding),
        binding.item
    )
}

fn tool_identity(tool: &ToolRef) -> &str {
    tool.qualified_name.as_deref().unwrap_or(&tool.name)
}

fn source_tool_identity(binding: &SourceToolBinding) -> &str {
    binding.qualified_name.as_deref().unwrap_or(&binding.name)
}

fn frame_span(frame: &EvalFrame) -> Span {
    match frame {
        EvalFrame::Call(frame) => frame.span,
        EvalFrame::Handler(frame) => frame.span,
        EvalFrame::ModelLoop(frame) => frame.pending.span,
        EvalFrame::SourceToolReturn(frame) => frame.model_loop.pending.span,
        _ => Span::empty(etas_core::SourceId(0), etas_core::TextSize(0)),
    }
}

fn machine_abort(message: impl Into<String>, span: Span) -> MachinePoll {
    MachinePoll::Fault(ExecutionFault::new(
        AnalysisDiagnosticCode::UnhandledRuntimeError,
        span,
        message,
    ))
}

fn format_host_error(error: &HostError) -> String {
    if error.details.is_empty() {
        return error.message.clone();
    }
    let details = error
        .details
        .iter()
        .map(|detail| format!("{}={}", detail.key, detail.value))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{} ({details})", error.message)
}

fn validate_host_value_schema(
    value: &HostValue,
    schema: &HostSchema,
    path: &str,
) -> Result<(), String> {
    if let HostValue::Json(json) = value {
        return validate_host_json_schema(json, schema, path);
    }
    match schema {
        HostSchema::Json => Ok(()),
        HostSchema::Unit => matches_schema(matches!(value, HostValue::Unit), path, "unit"),
        HostSchema::Bool => matches_schema(matches!(value, HostValue::Bool(_)), path, "bool"),
        HostSchema::Int => matches_schema(matches!(value, HostValue::Int(_)), path, "int"),
        HostSchema::UInt => matches_schema(matches!(value, HostValue::UInt(_)), path, "uint"),
        HostSchema::Float => matches_schema(matches!(value, HostValue::Float(_)), path, "float"),
        HostSchema::String => matches_schema(matches!(value, HostValue::String(_)), path, "string"),
        HostSchema::Bytes => matches_schema(matches!(value, HostValue::Bytes(_)), path, "bytes"),
        HostSchema::List(item_schema) => {
            let HostValue::List(values) = value else {
                return Err(format!("{path} must be a list"));
            };
            for (index, item) in values.iter().enumerate() {
                validate_host_value_schema(item, item_schema, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        HostSchema::Map { key, value: item } => {
            let HostValue::Map(entries) = value else {
                return Err(format!("{path} must be a map"));
            };
            for (index, (entry_key, entry_value)) in entries.iter().enumerate() {
                validate_host_value_schema(entry_key, key, &format!("{path}[{index}].key"))?;
                validate_host_value_schema(entry_value, item, &format!("{path}[{index}].value"))?;
            }
            Ok(())
        }
        HostSchema::Record(fields) => {
            let HostValue::Record(values) = value else {
                return Err(format!("{path} must be a record"));
            };
            validate_record_fields(values, fields, path, validate_host_value_schema)
        }
        HostSchema::Variant(variants) => {
            let HostValue::Variant { name, fields } = value else {
                return Err(format!("{path} must be a variant"));
            };
            let Some(variant) = variants.iter().find(|variant| variant.name == *name) else {
                return Err(format!("{path} has unknown variant `{name}`"));
            };
            if fields.len() != variant.fields.len() {
                return Err(format!(
                    "{path}.{name} expects {} field(s), got {}",
                    variant.fields.len(),
                    fields.len()
                ));
            }
            for (index, (field, schema)) in fields.iter().zip(&variant.fields).enumerate() {
                validate_host_value_schema(field, schema, &format!("{path}.{name}[{index}]"))?;
            }
            Ok(())
        }
    }
}

fn validate_record_fields<T>(
    values: &[(String, T)],
    fields: &[etas_host::HostFieldSchema],
    path: &str,
    validate: fn(&T, &HostSchema, &str) -> Result<(), String>,
) -> Result<(), String> {
    for field in fields {
        match values.iter().find(|(name, _)| name == &field.name) {
            Some((_, value)) => validate(value, &field.schema, &format!("{path}.{}", field.name))?,
            None if field.optional => {}
            None => return Err(format!("{path} is missing field `{}`", field.name)),
        }
    }
    for (name, _) in values {
        if !fields.iter().any(|field| &field.name == name) {
            return Err(format!("{path} has unexpected field `{name}`"));
        }
    }
    Ok(())
}

fn validate_host_json_schema(
    value: &HostJsonValue,
    schema: &HostSchema,
    path: &str,
) -> Result<(), String> {
    match schema {
        HostSchema::Json => Ok(()),
        HostSchema::Unit => matches_schema(matches!(value, HostJsonValue::Null), path, "unit"),
        HostSchema::Bool => matches_schema(matches!(value, HostJsonValue::Bool(_)), path, "bool"),
        HostSchema::Int => matches_schema(
            matches!(value, HostJsonValue::Number(number) if number.fract() == 0.0),
            path,
            "int",
        ),
        HostSchema::UInt => matches_schema(
            matches!(value, HostJsonValue::Number(number) if number.fract() == 0.0 && *number >= 0.0),
            path,
            "uint",
        ),
        HostSchema::Float => matches_schema(
            matches!(value, HostJsonValue::Number(number) if number.is_finite()),
            path,
            "float",
        ),
        HostSchema::String => {
            matches_schema(matches!(value, HostJsonValue::String(_)), path, "string")
        }
        HostSchema::Bytes => {
            let HostJsonValue::Array(values) = value else {
                return Err(format!("{path} must be bytes"));
            };
            for (index, byte) in values.iter().enumerate() {
                let HostJsonValue::Number(number) = byte else {
                    return Err(format!("{path}[{index}] must be a byte"));
                };
                if number.fract() != 0.0 || !(0.0..=255.0).contains(number) {
                    return Err(format!("{path}[{index}] must be a byte"));
                }
            }
            Ok(())
        }
        HostSchema::List(item_schema) => {
            let HostJsonValue::Array(values) = value else {
                return Err(format!("{path} must be a list"));
            };
            for (index, item) in values.iter().enumerate() {
                validate_host_json_schema(item, item_schema, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        HostSchema::Map { key, value: item } => {
            let HostJsonValue::Array(entries) = value else {
                return Err(format!("{path} must be a map entry list"));
            };
            for (index, entry) in entries.iter().enumerate() {
                let HostJsonValue::Array(pair) = entry else {
                    return Err(format!("{path}[{index}] must be a [key, value] pair"));
                };
                if pair.len() != 2 {
                    return Err(format!("{path}[{index}] must be a [key, value] pair"));
                }
                validate_host_json_schema(&pair[0], key, &format!("{path}[{index}].key"))?;
                validate_host_json_schema(&pair[1], item, &format!("{path}[{index}].value"))?;
            }
            Ok(())
        }
        HostSchema::Record(fields) => {
            let HostJsonValue::Object(values) = value else {
                return Err(format!("{path} must be a record"));
            };
            validate_record_fields(values, fields, path, validate_host_json_schema)
        }
        HostSchema::Variant(variants) => {
            let HostJsonValue::Object(values) = value else {
                return Err(format!("{path} must be a variant object"));
            };
            let Some(HostJsonValue::String(name)) = values
                .iter()
                .find(|(field, _)| field == "name")
                .map(|(_, value)| value)
            else {
                return Err(format!("{path} variant is missing string field `name`"));
            };
            let Some(HostJsonValue::Array(fields)) = values
                .iter()
                .find(|(field, _)| field == "fields")
                .map(|(_, value)| value)
            else {
                return Err(format!("{path} variant is missing array field `fields`"));
            };
            let Some(variant) = variants.iter().find(|variant| variant.name == *name) else {
                return Err(format!("{path} has unknown variant `{name}`"));
            };
            if fields.len() != variant.fields.len() {
                return Err(format!(
                    "{path}.{name} expects {} field(s), got {}",
                    variant.fields.len(),
                    fields.len()
                ));
            }
            for (index, (field, schema)) in fields.iter().zip(&variant.fields).enumerate() {
                validate_host_json_schema(field, schema, &format!("{path}.{name}[{index}]"))?;
            }
            Ok(())
        }
    }
}

fn matches_schema(matches: bool, path: &str, expected: &str) -> Result<(), String> {
    if matches {
        Ok(())
    } else {
        Err(format!("{path} must be {expected}"))
    }
}

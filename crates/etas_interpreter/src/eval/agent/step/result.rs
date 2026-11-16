use super::*;
use crate::eval::tool::{ToolInvocationError, ToolRejection};
use etas_host::{ModelOutput, ModelTermination, ModelUsage};

impl EvalContext<'_> {
    pub(in crate::eval) fn step_result_value(
        &mut self,
        model: &PendingModel,
        response: ModelOutput,
        usage: Option<ModelUsage>,
    ) -> Result<InterpValue, ExecutionFault> {
        let missing = |reason: String| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                model.span,
                reason,
            )
        };
        let ModelDecode::Step { result, output } = model.decode else {
            return Err(missing(
                "step response requires its checked result ABI".into(),
            ));
        };
        let content = response
            .message
            .content
            .iter()
            .map(|item| match item {
                ModelContent::Text(text) => variant("Text", vec![HostValue::String(text.clone())]),
                ModelContent::Value(value) => variant("Value", vec![value.clone()]),
            })
            .collect();
        let turn = HostValue::Record(vec![
            (
                "request_id".into(),
                option(response.provider_request_id.clone().map(HostValue::String)),
            ),
            ("content".into(), HostValue::List(content)),
            (
                "provider_data".into(),
                HostValue::Map(
                    response
                        .message
                        .provider_data
                        .iter()
                        .map(|(key, value)| (HostValue::String(key.clone()), value.clone()))
                        .collect(),
                ),
            ),
        ]);
        let decision = match &response.termination {
            ModelTermination::Truncated => stopped("Truncated", Vec::new()),
            ModelTermination::Refused(reason) => {
                stopped("Refused", vec![HostValue::String(reason.clone())])
            }
            ModelTermination::InvalidToolCall(reason) => stopped(
                "InvalidToolCall",
                vec![diagnostic("invalid_tool_call", reason.clone())],
            ),
            ModelTermination::InvalidOutput(reason) => stopped(
                "InvalidOutput",
                vec![diagnostic("invalid_output", reason.clone())],
            ),
            ModelTermination::Complete if !response.message.tool_calls.is_empty() => {
                match self.step_tool_calls(model, &response.message.tool_calls) {
                    Ok(calls) => variant("CallTools", vec![HostValue::List(calls)]),
                    Err(ToolInvocationError::Rejected(error)) => stopped(
                        "InvalidToolCall",
                        vec![diagnostic(error.code, error.message)],
                    ),
                    Err(ToolInvocationError::Fault(fault)) => return Err(fault),
                }
            }
            ModelTermination::Complete => {
                let value = if self.type_is_string(output) {
                    let mut text = String::new();
                    let mut valid = true;
                    for content in &response.message.content {
                        match content {
                            ModelContent::Text(value) => text.push_str(value),
                            ModelContent::Value(_) => valid = false,
                        }
                    }
                    if valid {
                        Ok(InterpValue::String(text.into()))
                    } else {
                        Err("string output contains structured content".into())
                    }
                } else {
                    self.typed_model_result_value(model, response, output)
                        .map_err(|error| error.message)
                };
                match value {
                    Ok(value) => variant(
                        "Final",
                        vec![interp_to_host_value(&value).map_err(missing)?],
                    ),
                    Err(reason) => {
                        stopped("InvalidOutput", vec![diagnostic("invalid_output", reason)])
                    }
                }
            }
        };
        let value = HostValue::Record(vec![
            ("turn".into(), turn),
            ("decision".into(), decision),
            ("usage".into(), option(usage.map(usage_value))),
        ]);
        crate::eval::host_value::decode_checked_host_value(value, result, self.checked)
            .map_err(missing)
    }

    fn step_tool_calls(
        &mut self,
        model: &PendingModel,
        calls: &[etas_host::ModelToolCall],
    ) -> Result<Vec<HostValue>, ToolInvocationError> {
        let missing = |message: &str| {
            ToolInvocationError::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                model.span,
                message,
            ))
        };
        let rejected = |message: String| {
            ToolInvocationError::Rejected(ToolRejection {
                code: "invalid_tool_call",
                message,
            })
        };
        let mut seen = std::collections::BTreeSet::new();
        let ModelDecode::Step { result, .. } = model.decode else {
            return Err(missing("tool requests require a checked step result"));
        };
        let Some(Type::Applied { args, .. }) = self.checked.type_store.get(result) else {
            return Err(missing("step has no checked context ABI"));
        };
        let context = self
            .task
            .agent_context
            .as_ref()
            .ok_or_else(|| missing("step has no active method context"))?;
        if !args
            .first()
            .is_some_and(|ty| self.agent_context_matches(*ty, context.method))
        {
            return Err(missing("step has no checked method context"));
        }
        let agent = self
            .checked
            .types
            .agent_methods
            .get(&context.method)
            .ok_or_else(|| missing("step method is missing"))?
            .agent;
        let tools = self
            .agent_tool_surface(agent)
            .map_err(ToolInvocationError::Fault)?;
        calls
            .iter()
            .map(|call| {
                if call.id.is_empty() || !seen.insert(&call.id) {
                    return Err(rejected("tool call IDs must be nonempty and unique".into()));
                }
                let binding = tools
                    .iter()
                    .find(|tool| tool.schema.tool.name == call.tool)
                    .ok_or_else(|| {
                        rejected(format!(
                            "tool `{}` is not a registered native tool in this context",
                            call.tool
                        ))
                    })?;
                self.storage_limits
                    .result_value_size(&call.args)
                    .map_err(|error| rejected(error.message))?;
                crate::eval::tool::schema::validate_host_value_schema(
                    &call.args,
                    &binding.schema.input,
                    "args",
                )
                .map_err(rejected)?;
                let (id, revision) = self
                    .canonical_tool_identity(binding.symbol)
                    .map_err(|message| missing(&message))?;
                Ok(HostValue::Record(vec![
                    ("id".into(), HostValue::String(call.id.clone())),
                    (
                        "tool".into(),
                        HostValue::Record(vec![
                            ("id".into(), HostValue::String(id)),
                            ("revision".into(), HostValue::String(revision)),
                        ]),
                    ),
                    ("args".into(), call.args.clone()),
                ]))
            })
            .collect()
    }
}

fn stopped(name: &str, fields: Vec<HostValue>) -> HostValue {
    variant("Stopped", vec![variant(name, fields)])
}

fn diagnostic(code: &str, message: String) -> HostValue {
    HostValue::Record(vec![
        ("code".into(), HostValue::String(code.into())),
        ("message".into(), HostValue::String(message)),
        ("path".into(), option(None)),
    ])
}

fn usage_value(usage: ModelUsage) -> HostValue {
    HostValue::Record(vec![
        (
            "input_tokens".into(),
            option(
                usage
                    .input_tokens
                    .map(|value| HostValue::UInt(value.into())),
            ),
        ),
        (
            "output_tokens".into(),
            option(
                usage
                    .output_tokens
                    .map(|value| HostValue::UInt(value.into())),
            ),
        ),
        (
            "cost".into(),
            option(usage.cost.map(|cost| {
                HostValue::Record(vec![
                    ("micros".into(), HostValue::UInt(cost.micros)),
                    ("currency".into(), HostValue::String(cost.currency)),
                ])
            })),
        ),
    ])
}

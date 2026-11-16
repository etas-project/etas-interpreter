use super::*;
use crate::value::{
    ConversationValue, MessageRoleValue,
    conversation::{self, protocol},
};
use etas_host::{ModelToolCall, host_value_to_json};

fn message(role: ModelRole, content: Vec<ModelContent>) -> ModelMessage {
    ModelMessage {
        provider_data: Vec::new(),
        role,
        content,
        tool_call_id: None,
        tool_calls: Vec::new(),
    }
}

impl EvalContext<'_> {
    pub(super) fn step_messages(
        &self,
        conversation: &ConversationValue,
        tools: &[crate::eval::tool::NativeToolBinding],
        span: Span,
    ) -> Result<Vec<ModelMessage>, ExecutionFault> {
        let invalid = |reason: String| {
            ExecutionFault::new(AnalysisDiagnosticCode::InvalidArguments, span, reason)
        };
        conversation::validate(conversation, &self.storage_limits).map_err(invalid)?;
        let mut messages = Vec::new();
        if let Some(context) = &conversation.selected_context {
            messages.push(message(
                ModelRole::System,
                vec![ModelContent::Text(context.content.text.clone())],
            ));
        }
        for item in &conversation.messages {
            let role = match item.role {
                MessageRoleValue::System => ModelRole::System,
                MessageRoleValue::User => ModelRole::User,
                MessageRoleValue::Assistant => ModelRole::Assistant,
                MessageRoleValue::Tool => ModelRole::Tool,
            };
            let payload = interp_to_host_value(&item.payload).map_err(invalid)?;
            messages.push(message(role, vec![ModelContent::Value(payload)]));
        }
        let Some(state) = &conversation.protocol else {
            return Ok(messages);
        };
        if protocol::validate(state, self.storage_limits.max_scan_rows)
            .map_err(|error| invalid(error.message))?
            .is_some()
        {
            return Err(invalid(
                "step requires results for all pending tool calls".into(),
            ));
        }
        let (prompt, turns) = protocol::parts(state).map_err(|error| invalid(error.message))?;
        for item in prompt {
            let role = match item.role {
                crate::value::PromptRole::System => ModelRole::System,
                crate::value::PromptRole::Assistant => ModelRole::Assistant,
                crate::value::PromptRole::User | crate::value::PromptRole::Data => ModelRole::User,
            };
            messages.push(message(
                role,
                vec![ModelContent::Text(item.text.to_string())],
            ));
        }
        let events = turns.iter().collect::<Vec<_>>();
        for event in events.into_iter().rev() {
            let field =
                |value, name| protocol::field(value, name).map_err(|error| invalid(error.message));
            match event {
                InterpValue::Record(_) => {
                    let turn = field(event, "turn")?;
                    let InterpValue::Array(content) = field(turn, "content")? else {
                        return Err(invalid("assistant content must be an Array".into()));
                    };
                    let mut assistant = message(ModelRole::Assistant, Vec::new());
                    let metadata =
                        interp_to_host_value(field(turn, "provider_data")?).map_err(invalid)?;
                    let HostValue::Map(entries) = metadata else {
                        return Err(invalid("provider_data must be a Map".into()));
                    };
                    assistant.provider_data = entries
                        .into_iter()
                        .map(|(key, value)| match key {
                            HostValue::String(key) => Ok((key, value)),
                            _ => Err(invalid("provider metadata key must be text".into())),
                        })
                        .collect::<Result<_, _>>()?;
                    for content in content.borrow().iter() {
                        let content = match content {
                            InterpValue::Nominal { value, .. } => value.as_ref(),
                            value => value,
                        };
                        let InterpValue::Variant { name, fields } = content else {
                            return Err(invalid(
                                "assistant content is not a MessageContent".into(),
                            ));
                        };
                        match (name.as_str(), fields.as_slice()) {
                            ("Text", [InterpValue::String(text)]) => {
                                assistant.content.push(ModelContent::Text(text.to_string()))
                            }
                            ("Value", [value]) => assistant.content.push(ModelContent::Value(
                                interp_to_host_value(value).map_err(invalid)?,
                            )),
                            _ => return Err(invalid("invalid assistant MessageContent".into())),
                        }
                    }
                    let InterpValue::Array(calls) = field(event, "calls")? else {
                        return Err(invalid("assistant requests are not an Array".into()));
                    };
                    for call in calls.borrow().iter() {
                        let id = text(field(call, "id")?).map_err(invalid)?;
                        let reference = field(call, "tool")?;
                        let tool = self.resolve_step_tool_name(reference, tools, span)?;
                        assistant.tool_calls.push(ModelToolCall {
                            id,
                            tool,
                            args: interp_to_host_value(field(call, "args")?).map_err(invalid)?,
                        });
                    }
                    messages.push(assistant);
                }
                InterpValue::Array(results) => {
                    for result in results.borrow().iter() {
                        let id = text(field(result, "call_id")?).map_err(invalid)?;
                        let outcome = field(result, "result")?;
                        let value = match outcome {
                            InterpValue::Variant { name, fields }
                                if name == "Ok" && fields.len() == 1 =>
                            {
                                &fields[0]
                            }
                            InterpValue::Variant { name, fields }
                                if name == "Err" && fields.len() == 1 =>
                            {
                                outcome
                            }
                            _ => return Err(invalid("tool feedback is not a Result".into())),
                        };
                        let value = interp_to_host_value(value).map_err(invalid)?;
                        let json = host_value_to_json(&value)
                            .map_err(|error| invalid(error.to_string()))?;
                        let mut result =
                            message(ModelRole::Tool, vec![ModelContent::Text(json.to_string())]);
                        result.tool_call_id = Some(id);
                        messages.push(result);
                    }
                }
                InterpValue::Prompt(prompt) => {
                    for item in prompt {
                        let role = match item.role {
                            crate::value::PromptRole::System => ModelRole::System,
                            crate::value::PromptRole::Assistant => ModelRole::Assistant,
                            crate::value::PromptRole::User | crate::value::PromptRole::Data => {
                                ModelRole::User
                            }
                        };
                        messages.push(message(
                            role,
                            vec![ModelContent::Text(item.text.to_string())],
                        ));
                    }
                }
                _ => return Err(invalid("unknown conversation protocol event".into())),
            }
        }
        Ok(messages)
    }

    fn resolve_step_tool_name(
        &self,
        reference: &InterpValue,
        tools: &[crate::eval::tool::NativeToolBinding],
        span: Span,
    ) -> Result<String, ExecutionFault> {
        let invalid = |reason: String| {
            ExecutionFault::new(AnalysisDiagnosticCode::InvalidArguments, span, reason)
        };
        let id = text(protocol::field(reference, "id").map_err(|error| invalid(error.message))?)
            .map_err(invalid)?;
        let revision =
            text(protocol::field(reference, "revision").map_err(|error| invalid(error.message))?)
                .map_err(invalid)?;
        for tool in tools {
            let (expected, snapshot) =
                self.canonical_tool_identity(tool.symbol).map_err(invalid)?;
            if id == expected && revision == snapshot {
                return Ok(tool.schema.tool.name.clone());
            }
        }
        Err(invalid(
            "conversation refers to a stale or out-of-surface native tool".into(),
        ))
    }
}

fn text(value: &InterpValue) -> Result<String, String> {
    match value {
        InterpValue::String(value) => Ok(value.to_string()),
        _ => Err("protocol text field is not a string".into()),
    }
}

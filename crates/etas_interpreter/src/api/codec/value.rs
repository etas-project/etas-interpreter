use super::*;

fn numeric_value_json(value: crate::value::NumericValue) -> Value {
    use crate::value::NumericValue;

    let primitive = value.primitive().source_name();
    match value {
        NumericValue::F32(bits) => json!({
            "kind": "number",
            "type": primitive,
            "bits": bits.to_string(),
        }),
        NumericValue::F64(bits) => json!({
            "kind": "number",
            "type": primitive,
            "bits": bits.to_string(),
        }),
        value => json!({
            "kind": "number",
            "type": primitive,
            "value": value.display_value(),
        }),
    }
}

fn numeric_value_from_json(
    value: &Value,
) -> Result<crate::value::NumericValue, InterpreterCodecError> {
    use crate::value::NumericValue;

    let primitive = required_str(value, "type")?;
    match primitive {
        "f32" => required_str(value, "bits")?
            .parse::<u32>()
            .map(NumericValue::F32)
            .map_err(|_| InterpreterCodecError::new("numeric f32 bits are invalid")),
        "f64" => required_str(value, "bits")?
            .parse::<u64>()
            .map(NumericValue::F64)
            .map_err(|_| InterpreterCodecError::new("numeric f64 bits are invalid")),
        name => {
            let primitive = primitive_type_from_abi_name(name).ok_or_else(|| {
                InterpreterCodecError::new(format!("unknown numeric ABI type `{name}`"))
            })?;
            NumericValue::parse_integer(required_str(value, "value")?, primitive).map_err(|_| {
                InterpreterCodecError::new(format!(
                    "numeric value is invalid for ABI type `{name}`"
                ))
            })
        }
    }
}

fn primitive_type_from_abi_name(name: &str) -> Option<etas_types::PrimitiveType> {
    use etas_types::PrimitiveType;
    Some(match name {
        "i8" => PrimitiveType::I8,
        "i16" => PrimitiveType::I16,
        "i32" => PrimitiveType::I32,
        "i64" => PrimitiveType::I64,
        "i128" => PrimitiveType::I128,
        "isize" => PrimitiveType::ISize,
        "u8" => PrimitiveType::U8,
        "u16" => PrimitiveType::U16,
        "u32" => PrimitiveType::U32,
        "u64" => PrimitiveType::U64,
        "u128" => PrimitiveType::U128,
        "usize" => PrimitiveType::USize,
        _ => return None,
    })
}

pub fn value_json(value: &InterpValue) -> Value {
    match value {
        InterpValue::Unit => json!({ "kind": "unit" }),
        InterpValue::Bool(value) => json!({ "kind": "bool", "value": value }),
        InterpValue::Number(value) => numeric_value_json(*value),
        InterpValue::String(value) => json!({ "kind": "string", "value": value }),
        InterpValue::Bytes(value) => json!({ "kind": "bytes", "value": value }),
        InterpValue::Json(value) => json!({
            "kind": "json",
            "value": host_json_support_value_json(value),
        }),
        InterpValue::Nominal { ty, value } => json!({
            "kind": "nominal",
            "ty": ty.0,
            "value": value_json(value),
        }),
        InterpValue::Trust { wrapper, value } => json!({
            "kind": "trust",
            "wrapper": value_codec::trust_wrapper_json(*wrapper),
            "value": value_json(value),
        }),
        InterpValue::Prompt(messages) => json!({
            "kind": "prompt",
            "messages": messages.iter().map(|message| {
                json!({
                    "role": value_codec::prompt_role_json(message.role),
                    "text": message.text,
                    "trust": message.trust.map(value_codec::trust_wrapper_json),
                })
            }).collect::<Vec<_>>()
        }),
        InterpValue::Message(message) => json!({
            "kind": "message",
            "id": message.id,
            "from": message.from,
            "to": message.to,
            "role": value_codec::message_role_json(message.role),
            "session": message.session,
            "created_at": message.created_at,
            "payload": value_json(&message.payload),
            "provenance": message.provenance.as_ref().map(provenance_json),
        }),
        InterpValue::Conversation(conversation) => json!({
            "kind": "conversation",
            "session": conversation.session,
            "messages": conversation.messages.iter().map(|message| {
                value_json(&InterpValue::Message(message.clone()))
            }).collect::<Vec<_>>(),
            "summary": conversation.summary.as_ref().map(|summary| {
                json!({
                    "text": summary.text,
                    "message_count": summary.message_count,
                })
            }),
            "cursor": conversation.cursor,
        }),
        InterpValue::Provenance(provenance) => json!({
            "kind": "provenance",
            "value": provenance_json(provenance),
        }),
        InterpValue::ModelResponse(response) => json!({
            "kind": "model_response",
            "id": response.id,
            "message": model_message_json(&response.message),
            "tool_calls": response.tool_calls.iter().map(model_tool_call_json).collect::<Vec<_>>(),
            "usage": response.usage.as_ref().map(|usage| {
                json!({
                    "input_tokens": usage.input_tokens,
                    "output_tokens": usage.output_tokens,
                })
            }),
        }),
        InterpValue::Command {
            argv,
            env,
            cwd,
            stdin,
        } => json!({
            "kind": "command",
            "argv": argv,
            "env": env.iter().map(|(key, value)| {
                json!({ "key": key, "value": value })
            }).collect::<Vec<_>>(),
            "cwd": cwd,
            "stdin": stdin,
        }),
        InterpValue::CommandResult {
            exit_code,
            stdout,
            stderr,
        } => json!({
            "kind": "command_result",
            "exit_code": exit_code,
            "stdout": stdout,
            "stderr": stderr,
        }),
        InterpValue::Tuple(values) => {
            json!({ "kind": "tuple", "values": values.iter().map(value_json).collect::<Vec<_>>() })
        }
        InterpValue::Array(values) => json!({
            "kind": "array",
            "values": values.borrow().iter().map(value_json).collect::<Vec<_>>()
        }),
        InterpValue::List(values) => json!({
            "kind": "list",
            "values": values.borrow().iter().map(value_json).collect::<Vec<_>>()
        }),
        InterpValue::Slice(values) => json!({
            "kind": "slice",
            "values": values.borrow().iter().map(value_json).collect::<Vec<_>>()
        }),
        InterpValue::Map(entries) => json!({
            "kind": "map",
            "entries": entries.borrow().iter().map(|(key, value)| {
                json!({ "key": value_json(key), "value": value_json(value) })
            }).collect::<Vec<_>>()
        }),
        InterpValue::Set(values) => json!({
            "kind": "set",
            "values": values.borrow().iter().map(value_json).collect::<Vec<_>>()
        }),
        InterpValue::Deque(values) => json!({
            "kind": "deque",
            "values": values.borrow().iter().map(value_json).collect::<Vec<_>>()
        }),
        InterpValue::Queue(values) => json!({
            "kind": "queue",
            "values": values.borrow().iter().map(value_json).collect::<Vec<_>>()
        }),
        InterpValue::Stack(values) => json!({
            "kind": "stack",
            "values": values.borrow().iter().map(value_json).collect::<Vec<_>>()
        }),
        InterpValue::PriorityQueue(entries) => json!({
            "kind": "priority_queue",
            "entries": entries.borrow().iter().map(|(priority, value)| {
                json!({ "priority": value_json(priority), "value": value_json(value) })
            }).collect::<Vec<_>>()
        }),
        InterpValue::OrderedMap(entries) => json!({
            "kind": "ordered_map",
            "entries": entries.borrow().iter().map(|(key, value)| {
                json!({ "key": value_json(key), "value": value_json(value) })
            }).collect::<Vec<_>>()
        }),
        InterpValue::OrderedSet(values) => json!({
            "kind": "ordered_set",
            "values": values.borrow().iter().map(value_json).collect::<Vec<_>>()
        }),
        InterpValue::Range(range) => json!({
            "kind": "range",
            "start": value_json(&range.start),
            "end": value_json(&range.end),
            "bounds": value_codec::range_bounds_json(range.bounds),
        }),
        InterpValue::Record(fields) => json!({
            "kind": "record",
            "fields": fields.borrow().iter().map(|(name, value)| {
                json!({ "name": name, "value": value_json(value) })
            }).collect::<Vec<_>>()
        }),
        InterpValue::Variant { name, fields } => json!({
            "kind": "variant",
            "name": name,
            "fields": fields.iter().map(value_json).collect::<Vec<_>>()
        }),
        InterpValue::OptionNone => json!({ "kind": "option_none" }),
        InterpValue::OptionSome(value) => {
            json!({ "kind": "option_some", "value": value_json(value) })
        }
        InterpValue::Callable(target) => {
            json!({
                "kind": "callable",
                "target": machine::call_target_snapshot(target),
            })
        }
        InterpValue::Handler {
            fact_expr,
            handlers,
        } => json!({
            "kind": "handler",
            "fact_expr": fact_expr.0,
            "handlers": handlers.iter().map(handler_arm_json).collect::<Vec<_>>()
        }),
        InterpValue::HostHandle(handle) => json!({
            "kind": "host_handle",
            "handle_kind": handle.kind_name(),
        }),
        InterpValue::ResourceHandle {
            name,
            stable_id,
            ty,
        } => {
            json!({ "kind": "resource_handle", "name": name, "stable_id": stable_id, "ty": ty.0 })
        }
        InterpValue::WorkspacePath(path) => json!({
            "kind": "workspace_path",
            "region": path.region.as_str(),
            "relative": path.relative.to_string_lossy(),
        }),
        InterpValue::MemoryStore {
            region_stable_id,
            path,
            key_type,
            value_type,
        } => json!({
            "kind": "memory_store",
            "region_stable_id": region_stable_id,
            "path": path,
            "key_type": key_type.0,
            "value_type": value_type.0,
        }),
        InterpValue::MemorySelection {
            region_stable_id,
            path,
            key_type,
            value_type,
            kind,
            predicate,
            limit,
        } => json!({
            "kind": "memory_selection",
            "region_stable_id": region_stable_id,
            "path": path,
            "key_type": key_type.0,
            "value_type": value_type.0,
            "selection": value_codec::memory_selection_kind_json(kind),
            "predicate": predicate.as_ref().map(|value| value_json(value)),
            "limit": limit,
        }),
    }
}

pub(crate) fn host_value_json(value: &HostValue) -> Value {
    match value {
        HostValue::Unit => json!({ "kind": "unit" }),
        HostValue::Bool(value) => json!({ "kind": "bool", "value": value }),
        HostValue::Int(value) => json!({ "kind": "int", "value": value.to_string() }),
        HostValue::UInt(value) => json!({ "kind": "uint", "value": value.to_string() }),
        HostValue::Float(value) => json!({ "kind": "float_bits", "value": value.to_bits() }),
        HostValue::String(value) => json!({ "kind": "string", "value": value }),
        HostValue::Bytes(value) => json!({ "kind": "bytes", "value": value }),
        HostValue::List(values) => json!({
            "kind": "list",
            "values": values.iter().map(host_value_json).collect::<Vec<_>>(),
        }),
        HostValue::Map(entries) => json!({
            "kind": "map",
            "entries": entries.iter().map(|(key, value)| {
                json!({ "key": host_value_json(key), "value": host_value_json(value) })
            }).collect::<Vec<_>>(),
        }),
        HostValue::Record(fields) => json!({
            "kind": "record",
            "fields": fields.iter().map(|(name, value)| {
                json!({ "name": name, "value": host_value_json(value) })
            }).collect::<Vec<_>>(),
        }),
        HostValue::Variant { name, fields } => json!({
            "kind": "variant",
            "name": name,
            "fields": fields.iter().map(host_value_json).collect::<Vec<_>>(),
        }),
        HostValue::Json(value) => json!({
            "kind": "json",
            "value": host_json_value_json(value),
        }),
    }
}

pub(crate) fn host_value_from_json(value: &Value) -> Result<HostValue, InterpreterCodecError> {
    match required_str(value, "kind")? {
        "unit" => Ok(HostValue::Unit),
        "bool" => Ok(HostValue::Bool(required_bool(value, "value")?)),
        "int" => Ok(HostValue::Int(
            required_str(value, "value")?
                .parse::<i128>()
                .map_err(|_| InterpreterCodecError::new("host int value must be an i128 string"))?,
        )),
        "uint" => Ok(HostValue::UInt(
            required_str(value, "value")?
                .parse::<u128>()
                .map_err(|_| InterpreterCodecError::new("host uint value must be a u128 string"))?,
        )),
        "float_bits" => Ok(HostValue::Float(f64::from_bits(required_u64(
            value, "value",
        )?))),
        "string" => Ok(HostValue::String(required_str(value, "value")?.to_owned())),
        "bytes" => Ok(HostValue::Bytes(byte_array(value, "value")?)),
        "list" => Ok(HostValue::List(host_values_from_array(value, "values")?)),
        "map" => Ok(HostValue::Map(
            required_array(value, "entries")?
                .iter()
                .map(|entry| {
                    Ok((
                        host_value_from_json(required_obj(entry, "key")?)?,
                        host_value_from_json(required_obj(entry, "value")?)?,
                    ))
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        )),
        "record" => Ok(HostValue::Record(
            required_array(value, "fields")?
                .iter()
                .map(|field| {
                    Ok((
                        required_str(field, "name")?.to_owned(),
                        host_value_from_json(required_obj(field, "value")?)?,
                    ))
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        )),
        "variant" => Ok(HostValue::Variant {
            name: required_str(value, "name")?.to_owned(),
            fields: host_values_from_array(value, "fields")?,
        }),
        "json" => Ok(HostValue::Json(host_json_value_from_json(required_obj(
            value, "value",
        )?)?)),
        other => Err(InterpreterCodecError::new(format!(
            "unsupported host value `{other}`"
        ))),
    }
}

pub(super) fn host_values_from_array(
    value: &Value,
    field: &'static str,
) -> Result<Vec<HostValue>, InterpreterCodecError> {
    required_array(value, field)?
        .iter()
        .map(host_value_from_json)
        .collect()
}

pub(super) fn host_json_value_json(value: &HostJsonValue) -> Value {
    match value {
        HostJsonValue::Null => json!({ "kind": "null" }),
        HostJsonValue::Bool(value) => json!({ "kind": "bool", "value": value }),
        HostJsonValue::Number(value) => json!({ "kind": "number_bits", "value": value.to_bits() }),
        HostJsonValue::String(value) => json!({ "kind": "string", "value": value }),
        HostJsonValue::Array(values) => json!({
            "kind": "array",
            "values": values.iter().map(host_json_value_json).collect::<Vec<_>>(),
        }),
        HostJsonValue::Object(entries) => json!({
            "kind": "object",
            "entries": entries.iter().map(|(key, value)| {
                json!({ "key": key, "value": host_json_value_json(value) })
            }).collect::<Vec<_>>(),
        }),
    }
}

pub(super) fn host_json_value_from_json(
    value: &Value,
) -> Result<HostJsonValue, InterpreterCodecError> {
    match required_str(value, "kind")? {
        "null" => Ok(HostJsonValue::Null),
        "bool" => Ok(HostJsonValue::Bool(required_bool(value, "value")?)),
        "number_bits" => Ok(HostJsonValue::Number(f64::from_bits(required_u64(
            value, "value",
        )?))),
        "string" => Ok(HostJsonValue::String(
            required_str(value, "value")?.to_owned(),
        )),
        "array" => Ok(HostJsonValue::Array(host_json_values_from_array(
            value, "values",
        )?)),
        "object" => Ok(HostJsonValue::Object(
            required_array(value, "entries")?
                .iter()
                .map(|entry| {
                    Ok((
                        required_str(entry, "key")?.to_owned(),
                        host_json_value_from_json(required_obj(entry, "value")?)?,
                    ))
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        )),
        other => Err(InterpreterCodecError::new(format!(
            "unsupported host json value `{other}`"
        ))),
    }
}

pub(super) fn host_json_values_from_array(
    value: &Value,
    field: &'static str,
) -> Result<Vec<HostJsonValue>, InterpreterCodecError> {
    required_array(value, field)?
        .iter()
        .map(host_json_value_from_json)
        .collect()
}

pub(super) fn provenance_json(provenance: &crate::value::ProvenanceValue) -> Value {
    json!({
        "trace_id": provenance.trace_id,
        "source": provenance.source,
    })
}

pub(super) fn provenance_from_json(
    value: &Value,
) -> Result<crate::value::ProvenanceValue, InterpreterCodecError> {
    Ok(crate::value::ProvenanceValue {
        trace_id: optional_string(value, "trace_id")?,
        source: optional_string(value, "source")?,
    })
}

pub(super) fn session_config_json(session: &crate::value::SessionConfigValue) -> Value {
    json!({
        "id": session.id,
        "context": session.context.as_deref().map(value_json),
        "retention": session.retention.as_deref().map(value_json),
        "compaction": session.compaction.as_deref().map(value_json),
    })
}

pub(super) fn model_message_json(message: &crate::value::ModelMessageValue) -> Value {
    json!({
        "role": value_codec::model_role_json(message.role),
        "content": message.content.iter().map(model_content_json).collect::<Vec<_>>(),
    })
}

pub(super) fn model_message_from_json(
    value: &Value,
) -> Result<crate::value::ModelMessageValue, InterpreterCodecError> {
    Ok(crate::value::ModelMessageValue {
        role: value_codec::model_role_from_json(required_str(value, "role")?)
            .map_err(InterpreterCodecError::new)?,
        content: required_array(value, "content")?
            .iter()
            .map(model_content_from_json)
            .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
    })
}

pub(super) fn model_content_json(content: &crate::value::ModelContentValue) -> Value {
    match content {
        crate::value::ModelContentValue::Text(text) => {
            json!({ "kind": "text", "text": text })
        }
        crate::value::ModelContentValue::Value(value) => {
            json!({ "kind": "value", "value": host_support_value_json(value) })
        }
    }
}

pub(super) fn model_content_from_json(
    value: &Value,
) -> Result<crate::value::ModelContentValue, InterpreterCodecError> {
    match required_str(value, "kind")? {
        "text" => Ok(crate::value::ModelContentValue::Text(
            required_str(value, "text")?.to_owned(),
        )),
        "value" => Ok(crate::value::ModelContentValue::Value(
            host_support_value_from_json(required_obj(value, "value")?)?,
        )),
        other => Err(InterpreterCodecError::new(format!(
            "unsupported model content `{other}`"
        ))),
    }
}

pub(super) fn model_tool_call_json(call: &crate::value::ModelToolCallValue) -> Value {
    json!({
        "id": call.id,
        "tool": call.tool,
        "args": host_support_value_json(&call.args),
    })
}

pub(super) fn model_tool_call_from_json(
    value: &Value,
) -> Result<crate::value::ModelToolCallValue, InterpreterCodecError> {
    Ok(crate::value::ModelToolCallValue {
        id: required_str(value, "id")?.to_owned(),
        tool: required_str(value, "tool")?.to_owned(),
        args: host_support_value_from_json(required_obj(value, "args")?)?,
    })
}

pub(super) fn model_response_from_json(
    value: &Value,
) -> Result<crate::value::ModelResponseValue, InterpreterCodecError> {
    Ok(crate::value::ModelResponseValue {
        id: required_u32(value, "id")?,
        message: model_message_from_json(required_obj(value, "message")?)?,
        tool_calls: required_array(value, "tool_calls")?
            .iter()
            .map(model_tool_call_from_json)
            .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        usage: match value.get("usage") {
            Some(Value::Null) | None => None,
            Some(value) => Some(crate::value::ModelUsageValue {
                input_tokens: required_u64(value, "input_tokens")?,
                output_tokens: required_u64(value, "output_tokens")?,
            }),
        },
    })
}

pub(super) fn host_support_value_json(value: &crate::value::HostSupportValue) -> Value {
    match value {
        crate::value::HostSupportValue::Unit => json!({ "kind": "unit" }),
        crate::value::HostSupportValue::Bool(value) => {
            json!({ "kind": "bool", "value": value })
        }
        crate::value::HostSupportValue::Int(value) => {
            json!({ "kind": "int", "value": value })
        }
        crate::value::HostSupportValue::UInt(value) => {
            json!({ "kind": "uint", "value": value })
        }
        crate::value::HostSupportValue::FloatBits(value) => {
            json!({ "kind": "float_bits", "value": value })
        }
        crate::value::HostSupportValue::String(value) => {
            json!({ "kind": "string", "value": value })
        }
        crate::value::HostSupportValue::Bytes(value) => {
            json!({ "kind": "bytes", "value": value })
        }
        crate::value::HostSupportValue::List(values) => json!({
            "kind": "list",
            "values": values.iter().map(host_support_value_json).collect::<Vec<_>>(),
        }),
        crate::value::HostSupportValue::Map(entries) => json!({
            "kind": "map",
            "entries": entries.iter().map(|(key, value)| {
                json!({
                    "key": host_support_value_json(key),
                    "value": host_support_value_json(value),
                })
            }).collect::<Vec<_>>(),
        }),
        crate::value::HostSupportValue::Record(fields) => json!({
            "kind": "record",
            "fields": fields.iter().map(|(name, value)| {
                json!({ "name": name, "value": host_support_value_json(value) })
            }).collect::<Vec<_>>(),
        }),
        crate::value::HostSupportValue::Variant { name, fields } => json!({
            "kind": "variant",
            "name": name,
            "fields": fields.iter().map(host_support_value_json).collect::<Vec<_>>(),
        }),
        crate::value::HostSupportValue::Json(value) => json!({
            "kind": "json",
            "value": host_json_support_value_json(value),
        }),
    }
}

pub(super) fn host_support_value_from_json(
    value: &Value,
) -> Result<crate::value::HostSupportValue, InterpreterCodecError> {
    match required_str(value, "kind")? {
        "unit" => Ok(crate::value::HostSupportValue::Unit),
        "bool" => Ok(crate::value::HostSupportValue::Bool(required_bool(
            value, "value",
        )?)),
        "int" => Ok(crate::value::HostSupportValue::Int(
            required_str(value, "value")?.to_owned(),
        )),
        "uint" => Ok(crate::value::HostSupportValue::UInt(
            required_str(value, "value")?.to_owned(),
        )),
        "float_bits" => Ok(crate::value::HostSupportValue::FloatBits(required_u64(
            value, "value",
        )?)),
        "string" => Ok(crate::value::HostSupportValue::String(
            required_str(value, "value")?.to_owned(),
        )),
        "bytes" => Ok(crate::value::HostSupportValue::Bytes(byte_array(
            value, "value",
        )?)),
        "list" => Ok(crate::value::HostSupportValue::List(
            host_support_values_from_array(value, "values")?,
        )),
        "map" => Ok(crate::value::HostSupportValue::Map(
            required_array(value, "entries")?
                .iter()
                .map(|entry| {
                    Ok((
                        host_support_value_from_json(required_obj(entry, "key")?)?,
                        host_support_value_from_json(required_obj(entry, "value")?)?,
                    ))
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        )),
        "record" => Ok(crate::value::HostSupportValue::Record(
            required_array(value, "fields")?
                .iter()
                .map(|field| {
                    Ok((
                        required_str(field, "name")?.to_owned(),
                        host_support_value_from_json(required_obj(field, "value")?)?,
                    ))
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        )),
        "variant" => Ok(crate::value::HostSupportValue::Variant {
            name: required_str(value, "name")?.to_owned(),
            fields: host_support_values_from_array(value, "fields")?,
        }),
        "json" => Ok(crate::value::HostSupportValue::Json(
            host_json_support_value_from_json(required_obj(value, "value")?)?,
        )),
        other => Err(InterpreterCodecError::new(format!(
            "unsupported host support value `{other}`"
        ))),
    }
}

pub(super) fn host_support_values_from_array(
    value: &Value,
    field: &'static str,
) -> Result<Vec<crate::value::HostSupportValue>, InterpreterCodecError> {
    required_array(value, field)?
        .iter()
        .map(host_support_value_from_json)
        .collect()
}

pub(super) fn host_json_support_value_json(value: &crate::value::HostJsonSupportValue) -> Value {
    match value {
        crate::value::HostJsonSupportValue::Null => json!({ "kind": "null" }),
        crate::value::HostJsonSupportValue::Bool(value) => {
            json!({ "kind": "bool", "value": value })
        }
        crate::value::HostJsonSupportValue::NumberBits(value) => {
            json!({ "kind": "number_bits", "value": value })
        }
        crate::value::HostJsonSupportValue::String(value) => {
            json!({ "kind": "string", "value": value })
        }
        crate::value::HostJsonSupportValue::Array(values) => json!({
            "kind": "array",
            "values": values.iter().map(host_json_support_value_json).collect::<Vec<_>>(),
        }),
        crate::value::HostJsonSupportValue::Object(entries) => json!({
            "kind": "object",
            "entries": entries.iter().map(|(key, value)| {
                json!({ "key": key, "value": host_json_support_value_json(value) })
            }).collect::<Vec<_>>(),
        }),
    }
}

pub(super) fn host_json_support_value_from_json(
    value: &Value,
) -> Result<crate::value::HostJsonSupportValue, InterpreterCodecError> {
    match required_str(value, "kind")? {
        "null" => Ok(crate::value::HostJsonSupportValue::Null),
        "bool" => Ok(crate::value::HostJsonSupportValue::Bool(required_bool(
            value, "value",
        )?)),
        "number_bits" => Ok(crate::value::HostJsonSupportValue::NumberBits(
            required_u64(value, "value")?,
        )),
        "string" => Ok(crate::value::HostJsonSupportValue::String(
            required_str(value, "value")?.to_owned(),
        )),
        "array" => Ok(crate::value::HostJsonSupportValue::Array(
            required_array(value, "values")?
                .iter()
                .map(host_json_support_value_from_json)
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        )),
        "object" => Ok(crate::value::HostJsonSupportValue::Object(
            required_array(value, "entries")?
                .iter()
                .map(|entry| {
                    Ok((
                        required_str(entry, "key")?.to_owned(),
                        host_json_support_value_from_json(required_obj(entry, "value")?)?,
                    ))
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        )),
        other => Err(InterpreterCodecError::new(format!(
            "unsupported host json support value `{other}`"
        ))),
    }
}

pub(super) fn values_from_array(
    value: &Value,
    field: &'static str,
) -> Result<Vec<InterpValue>, InterpreterCodecError> {
    required_array(value, field)?
        .iter()
        .map(value_from_json)
        .collect()
}

pub(crate) fn value_from_json(value: &Value) -> Result<InterpValue, InterpreterCodecError> {
    match required_str(value, "kind")? {
        "unit" => Ok(InterpValue::Unit),
        "bool" => Ok(InterpValue::Bool(required_bool(value, "value")?)),
        "number" => Ok(InterpValue::Number(numeric_value_from_json(value)?)),
        "string" => Ok(InterpValue::String(
            required_str(value, "value")?.to_owned(),
        )),
        "bytes" => Ok(InterpValue::Bytes(byte_array(value, "value")?)),
        "json" => Ok(InterpValue::Json(host_json_support_value_from_json(
            required_obj(value, "value")?,
        )?)),
        "trust" => Ok(InterpValue::Trust {
            wrapper: value_codec::trust_wrapper_from_json(required_str(value, "wrapper")?)
                .map_err(InterpreterCodecError::new)?,
            value: Box::new(value_from_json(required_obj(value, "value")?)?),
        }),
        "prompt" => Ok(InterpValue::Prompt(
            required_array(value, "messages")?
                .iter()
                .map(|message| {
                    Ok(crate::value::PromptMessage {
                        role: value_codec::prompt_role_from_json(required_str(message, "role")?)
                            .map_err(InterpreterCodecError::new)?,
                        text: required_str(message, "text")?.to_owned(),
                        trust: optional_string(message, "trust")?
                            .map(|wrapper| {
                                value_codec::trust_wrapper_from_json(&wrapper)
                                    .map_err(InterpreterCodecError::new)
                            })
                            .transpose()?,
                    })
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        )),
        "message" => {
            reject_unknown_fields(
                value,
                &[
                    "kind",
                    "id",
                    "from",
                    "to",
                    "role",
                    "session",
                    "created_at",
                    "payload",
                    "provenance",
                ],
                "message value",
            )?;
            let provenance = value
                .get("provenance")
                .ok_or_else(|| InterpreterCodecError::new("missing `provenance`"))?;
            Ok(InterpValue::Message(crate::value::MessageValue {
                id: required_str(value, "id")?.to_owned(),
                from: required_optional_string(value, "from")?,
                to: required_optional_string(value, "to")?,
                role: value_codec::message_role_from_json(required_str(value, "role")?)
                    .map_err(InterpreterCodecError::new)?,
                session: required_optional_string(value, "session")?,
                created_at: required_str(value, "created_at")?.to_owned(),
                payload: Box::new(value_from_json(required_obj(value, "payload")?)?),
                provenance: if provenance.is_null() {
                    None
                } else {
                    Some(provenance_from_json(provenance)?)
                },
            }))
        }
        "provenance" => Ok(InterpValue::Provenance(provenance_from_json(
            required_obj(value, "value")?,
        )?)),
        "model_response" => Ok(InterpValue::ModelResponse(model_response_from_json(value)?)),
        "command" => Ok(InterpValue::Command {
            argv: string_array(value, "argv")?,
            env: required_array(value, "env")?
                .iter()
                .map(|entry| {
                    Ok((
                        required_str(entry, "key")?.to_owned(),
                        required_str(entry, "value")?.to_owned(),
                    ))
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
            cwd: optional_string(value, "cwd")?,
            stdin: match value.get("stdin") {
                Some(Value::Null) | None => None,
                Some(_) => Some(byte_array(value, "stdin")?),
            },
        }),
        "command_result" => Ok(InterpValue::CommandResult {
            exit_code: required_i64(value, "exit_code").and_then(|exit_code| {
                i32::try_from(exit_code)
                    .map_err(|_| InterpreterCodecError::new("command exit_code must fit i32"))
            })?,
            stdout: byte_array(value, "stdout")?,
            stderr: byte_array(value, "stderr")?,
        }),
        "tuple" => Ok(InterpValue::Tuple(values_from_array(value, "values")?)),
        "array" => Ok(InterpValue::Array(
            values_from_array(value, "values")?.into(),
        )),
        "list" => Ok(InterpValue::List(
            values_from_array(value, "values")?.into(),
        )),
        "slice" => Ok(InterpValue::Slice(
            values_from_array(value, "values")?.into(),
        )),
        "map" => Ok(InterpValue::Map(
            required_array(value, "entries")?
                .iter()
                .map(|entry| {
                    Ok((
                        value_from_json(required_obj(entry, "key")?)?,
                        value_from_json(required_obj(entry, "value")?)?,
                    ))
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?
                .into(),
        )),
        "set" => Ok(InterpValue::Set(values_from_array(value, "values")?.into())),
        "deque" => Ok(InterpValue::Deque(
            values_from_array(value, "values")?.into(),
        )),
        "queue" => Ok(InterpValue::Queue(
            values_from_array(value, "values")?.into(),
        )),
        "stack" => Ok(InterpValue::Stack(
            values_from_array(value, "values")?.into(),
        )),
        "priority_queue" => Ok(InterpValue::PriorityQueue(
            required_array(value, "entries")?
                .iter()
                .map(|entry| {
                    Ok((
                        value_from_json(required_obj(entry, "priority")?)?,
                        value_from_json(required_obj(entry, "value")?)?,
                    ))
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?
                .into(),
        )),
        "ordered_map" => Ok(InterpValue::OrderedMap(
            required_array(value, "entries")?
                .iter()
                .map(|entry| {
                    Ok((
                        value_from_json(required_obj(entry, "key")?)?,
                        value_from_json(required_obj(entry, "value")?)?,
                    ))
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?
                .into(),
        )),
        "ordered_set" => Ok(InterpValue::OrderedSet(
            values_from_array(value, "values")?.into(),
        )),
        "range" => Ok(InterpValue::Range(crate::value::RangeValue {
            start: Box::new(value_from_json(required_obj(value, "start")?)?),
            end: Box::new(value_from_json(required_obj(value, "end")?)?),
            bounds: value_codec::range_bounds_from_json(required_str(value, "bounds")?)
                .map_err(InterpreterCodecError::new)?,
        })),
        "record" => Ok(InterpValue::Record(
            required_array(value, "fields")?
                .iter()
                .map(|field| {
                    Ok((
                        required_str(field, "name")?.to_owned(),
                        value_from_json(required_obj(field, "value")?)?,
                    ))
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?
                .into(),
        )),
        "nominal" => Ok(InterpValue::Nominal {
            ty: etas_types::TypeId(required_u32(value, "ty")?),
            value: Box::new(value_from_json(required_obj(value, "value")?)?),
        }),
        "variant" => Ok(InterpValue::Variant {
            name: required_str(value, "name")?.to_owned(),
            fields: values_from_array(value, "fields")?,
        }),
        "option_none" => Ok(InterpValue::OptionNone),
        "option_some" => Ok(InterpValue::OptionSome(Box::new(value_from_json(
            required_obj(value, "value")?,
        )?))),
        "handler" => Ok(InterpValue::Handler {
            fact_expr: HirExprId(required_u32(value, "fact_expr")?),
            handlers: required_array(value, "handlers")?
                .iter()
                .map(handler_arm_from_json)
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        }),
        "host_handle" => Err(InterpreterCodecError::new(
            "serialized host handles cannot be restored without a live host capability",
        )),
        "resource_handle" => Ok(InterpValue::ResourceHandle {
            name: required_str(value, "name")?.to_owned(),
            stable_id: required_str(value, "stable_id")?.to_owned(),
            ty: etas_types::TypeId(required_u32(value, "ty")?),
        }),
        "workspace_path" => Ok(InterpValue::WorkspacePath(
            etas_host::WorkspacePathRef::new(
                etas_host::WorkspaceRegionId::new(required_str(value, "region")?.to_owned())
                    .map_err(|error| InterpreterCodecError::new(error.message))?,
                required_str(value, "relative")?,
            )
            .map_err(|error| InterpreterCodecError::new(error.message))?,
        )),
        "memory_store" => Ok(InterpValue::MemoryStore {
            region_stable_id: required_str(value, "region_stable_id")?.to_owned(),
            path: string_array(value, "path")?,
            key_type: etas_types::TypeId(required_u32(value, "key_type")?),
            value_type: etas_types::TypeId(required_u32(value, "value_type")?),
        }),
        "memory_selection" => Ok(InterpValue::MemorySelection {
            region_stable_id: required_str(value, "region_stable_id")?.to_owned(),
            path: string_array(value, "path")?,
            key_type: etas_types::TypeId(required_u32(value, "key_type")?),
            value_type: etas_types::TypeId(required_u32(value, "value_type")?),
            kind: value_codec::memory_selection_kind_from_json(required_str(value, "selection")?)
                .map_err(InterpreterCodecError::new)?,
            predicate: match value.get("predicate") {
                Some(Value::Null) | None => None,
                Some(value) => Some(Box::new(value_from_json(value)?)),
            },
            limit: optional_u32(value, "limit")?,
        }),
        "callable" => Ok(InterpValue::Callable(
            machine::call_target_from_artifact_snapshot(required_obj(value, "target")?)
                .map_err(InterpreterCodecError::new)?,
        )),
        other => Err(InterpreterCodecError::new(format!(
            "unsupported serialized interpreter value `{other}`"
        ))),
    }
}

fn reject_unknown_fields(
    value: &Value,
    allowed: &[&str],
    context: &str,
) -> Result<(), InterpreterCodecError> {
    let fields = value
        .as_object()
        .ok_or_else(|| InterpreterCodecError::new(format!("{context} must be an object")))?;
    if let Some(field) = fields
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(InterpreterCodecError::new(format!(
            "{context} contains unknown field `{field}`"
        )));
    }
    Ok(())
}

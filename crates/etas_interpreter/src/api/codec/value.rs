use super::*;
mod decode;
pub(super) mod encode_host;
pub(super) mod encode_model;
mod encode_runtime;
#[cfg(test)]
#[path = "value/encode_runtime/tests.rs"]
mod runtime_encoding_tests;
mod scalar;
pub(super) mod session;
mod snapshot_support;

pub(super) use decode::json::decode as host_json_support_value_from_json;
pub(super) use encode_model::model_response_json;

pub(super) fn numeric_value_json(value: crate::value::NumericValue) -> Value {
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
    encode_runtime::encode(value)
}

pub(crate) fn host_value_json(value: &HostValue) -> Value {
    encode_host::encode(value)
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

pub(super) fn host_support_value_from_json(
    value: &Value,
) -> Result<crate::value::HostSupportValue, InterpreterCodecError> {
    decode::host::decode(value)
}

pub(super) fn values_from_array(
    limits: &etas_host::StorageLimits,
    value: &Value,
    field: &'static str,
) -> Result<Vec<InterpValue>, InterpreterCodecError> {
    required_array(value, field)?
        .iter()
        .map(|value| value_from_json_with_limits(limits, value))
        .collect()
}

pub(crate) fn value_from_json_with_limits(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<InterpValue, InterpreterCodecError> {
    decode::decode(limits, value)
}

pub(in crate::api::codec) fn snapshot_from_json_with_limits(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<crate::orchestration::ValueSnapshot, InterpreterCodecError> {
    decode::decode(limits, value)
}

fn decode_scalar_or_support(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<InterpValue, InterpreterCodecError> {
    match required_str(value, "kind")? {
        "message" => session::message_parts(limits, value, value_from_json_with_limits)
            .map(|message| InterpValue::Message(message.into())),
        "conversation" => {
            session::conversation_from_json(limits, value).map(InterpValue::Conversation)
        }
        "memory_selection" => Ok(InterpValue::MemorySelection {
            region_stable_id: required_str(value, "region_stable_id")?.to_owned(),
            path: string_array(value, "path")?,
            key_type: etas_types::TypeId(required_u32(value, "key_type")?),
            value_type: etas_types::TypeId(required_u32(value, "value_type")?),
            kind: value_codec::memory_selection_kind_from_json(required_str(value, "selection")?)
                .map_err(InterpreterCodecError::new)?,
            predicate: match value.get("predicate") {
                Some(Value::Null) | None => None,
                Some(value) => Some(Box::new(value_from_json_with_limits(limits, value)?)),
            },
            limit: optional_u32(value, "limit")?,
        }),
        "callable" => Ok(InterpValue::Callable(
            machine::call_target_from_artifact_snapshot(limits, required_obj(value, "target")?)
                .map_err(InterpreterCodecError::new)?,
        )),
        _ => scalar::decode(limits, value).map(Into::into),
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

#[cfg(test)]
pub(crate) fn value_from_json(value: &Value) -> Result<InterpValue, InterpreterCodecError> {
    value_from_json_with_limits(&etas_host::StorageLimits::default(), value)
}

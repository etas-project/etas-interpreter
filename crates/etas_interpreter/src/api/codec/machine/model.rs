use serde_json::{Value, json};

use super::value::{
    optional_string, optional_u32, required, required_bool, required_str, required_usize,
};

pub(super) fn model_policy_snapshot(policy: &crate::api::ModelExecutionPolicy) -> Value {
    json!({
        "provider": policy.provider.as_ref().map(|provider| provider.0.as_str()),
        "provider_capabilities": policy.provider_capabilities.map(|capabilities| json!({
            "forced_tool_output": capabilities.supports_forced_tool_output,
            "json_schema_response_format": capabilities.supports_json_schema_response_format,
            "plain_json_text_instruction": capabilities.supports_plain_json_text_instruction,
            "tool_call_loop": capabilities.supports_tool_call_loop,
            "required_tool_choice": capabilities.supports_required_tool_choice,
        })),
        "model": policy.model.0,
        "model_locked": policy.model_locked,
        "tools": policy.tools.iter().map(tool_schema_snapshot).collect::<Vec<_>>(),
        "tool_choice": model_tool_choice_snapshot(&policy.tool_choice),
        "policy_ref": policy.policy_ref.as_ref().map(crate::api::codec::host_value_json),
        "options": model_options_snapshot(&policy.options),
        "budget": policy.budget.as_ref().map(crate::api::codec::budget_json),
        "response_decode": match policy.response_decode {
            crate::api::ModelResponseDecodePolicy::String => "string",
            crate::api::ModelResponseDecodePolicy::ModelResponse => "model_response",
        },
        "max_tool_rounds": policy.max_tool_rounds,
    })
}

pub(super) fn model_policy_from_snapshot(
    value: &Value,
) -> Result<crate::api::ModelExecutionPolicy, String> {
    let provider_capabilities = match value.get("provider_capabilities") {
        Some(Value::Null) => None,
        Some(value) => Some(etas_host::ModelProviderCapabilities {
            supports_forced_tool_output: required_bool(value, "forced_tool_output")?,
            supports_json_schema_response_format: required_bool(
                value,
                "json_schema_response_format",
            )?,
            supports_plain_json_text_instruction: required_bool(
                value,
                "plain_json_text_instruction",
            )?,
            supports_tool_call_loop: required_bool(value, "tool_call_loop")?,
            supports_required_tool_choice: required_bool(value, "required_tool_choice")?,
        }),
        None => return Err("machine model policy is missing `provider_capabilities`".to_owned()),
    };
    let tools = required(value, "tools")?
        .as_array()
        .ok_or_else(|| "machine model policy `tools` must be an array".to_owned())?
        .iter()
        .map(tool_schema_from_snapshot)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(crate::api::ModelExecutionPolicy {
        provider: optional_string(value, "provider")?.map(etas_host::ModelProviderId),
        provider_capabilities,
        model: etas_host::ModelName(required_str(value, "model")?.to_owned()),
        model_locked: required_bool(value, "model_locked")?,
        tools,
        tool_choice: model_tool_choice_from_snapshot(required(value, "tool_choice")?)?,
        policy_ref: match value.get("policy_ref") {
            Some(Value::Null) => None,
            Some(value) => Some(
                crate::api::codec::host_value_from_json(value)
                    .map_err(|error| error.to_string())?,
            ),
            None => return Err("machine model policy is missing `policy_ref`".to_owned()),
        },
        options: model_options_from_snapshot(required(value, "options")?)?,
        budget: match value.get("budget") {
            Some(Value::Null) => None,
            Some(value) => Some(
                crate::api::codec::budget_from_json(value).map_err(|error| error.to_string())?,
            ),
            None => return Err("machine model policy is missing `budget`".to_owned()),
        },
        response_decode: match required_str(value, "response_decode")? {
            "string" => crate::api::ModelResponseDecodePolicy::String,
            "model_response" => crate::api::ModelResponseDecodePolicy::ModelResponse,
            other => return Err(format!("unknown machine model response decode `{other}`")),
        },
        max_tool_rounds: required_usize(value, "max_tool_rounds")?,
    })
}

pub(super) fn optional_model_policy(
    value: &Value,
    field: &str,
) -> Result<Option<crate::api::ModelExecutionPolicy>, String> {
    let Some(value) = value.get(field) else {
        return Err(format!("machine snapshot is missing `{field}`"));
    };
    if value.is_null() {
        return Ok(None);
    }
    model_policy_from_snapshot(value).map(Some)
}

pub(crate) fn model_options_snapshot(options: &etas_host::ModelOptions) -> Value {
    json!({
        "temperature": options.temperature,
        "max_output_tokens": options.max_output_tokens,
        "metadata": options.metadata.iter().map(|(key, value)| json!({
            "key": key,
            "value": crate::api::codec::host_value_json(value),
        })).collect::<Vec<_>>(),
    })
}

pub(crate) fn model_options_from_snapshot(
    value: &Value,
) -> Result<etas_host::ModelOptions, String> {
    let temperature = match value.get("temperature") {
        Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_f64()
                .ok_or_else(|| "machine model temperature must be a number".to_owned())?
                as f32,
        ),
        None => return Err("machine model options are missing `temperature`".to_owned()),
    };
    let max_output_tokens = match value.get("max_output_tokens") {
        Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or_else(|| "machine max output tokens must be a u64".to_owned())?,
        ),
        None => return Err("machine model options are missing `max_output_tokens`".to_owned()),
    };
    let metadata = required(value, "metadata")?
        .as_array()
        .ok_or_else(|| "machine model metadata must be an array".to_owned())?
        .iter()
        .map(|entry| {
            Ok((
                required_str(entry, "key")?.to_owned(),
                crate::api::codec::host_value_from_json(required(entry, "value")?)
                    .map_err(|error| error.to_string())?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(etas_host::ModelOptions {
        temperature,
        max_output_tokens,
        metadata,
    })
}

pub(crate) fn model_tool_choice_snapshot(choice: &etas_host::ModelToolChoice) -> Value {
    match choice {
        etas_host::ModelToolChoice::Auto => json!({ "kind": "auto" }),
        etas_host::ModelToolChoice::RequiredAny => json!({ "kind": "required_any" }),
        etas_host::ModelToolChoice::RequiredTool(name) => {
            json!({ "kind": "required_tool", "name": name })
        }
    }
}

pub(crate) fn model_tool_choice_from_snapshot(
    value: &Value,
) -> Result<etas_host::ModelToolChoice, String> {
    match required_str(value, "kind")? {
        "auto" => Ok(etas_host::ModelToolChoice::Auto),
        "required_any" => Ok(etas_host::ModelToolChoice::RequiredAny),
        "required_tool" => Ok(etas_host::ModelToolChoice::RequiredTool(
            required_str(value, "name")?.to_owned(),
        )),
        other => Err(format!("unknown machine model tool choice `{other}`")),
    }
}

pub(crate) fn tool_schema_snapshot(tool: &etas_host::ToolSchema) -> Value {
    json!({
        "tool": {
            "name": tool.tool.name,
            "qualified_name": tool.tool.qualified_name,
            "std_symbol": tool.tool.std_symbol.map(|symbol| symbol.0),
        },
        "input": host_schema_snapshot(&tool.input),
        "output": tool.output.as_ref().map(host_schema_snapshot),
    })
}

pub(crate) fn tool_schema_from_snapshot(value: &Value) -> Result<etas_host::ToolSchema, String> {
    let tool = required(value, "tool")?;
    Ok(etas_host::ToolSchema {
        tool: etas_host::ToolRef {
            name: required_str(tool, "name")?.to_owned(),
            qualified_name: optional_string(tool, "qualified_name")?,
            std_symbol: optional_u32(tool, "std_symbol")?.map(etas_std::StdSymbolId),
        },
        input: host_schema_from_snapshot(required(value, "input")?)?,
        output: match value.get("output") {
            Some(Value::Null) => None,
            Some(value) => Some(host_schema_from_snapshot(value)?),
            None => return Err("machine tool schema is missing `output`".to_owned()),
        },
    })
}

pub(crate) fn host_schema_snapshot(schema: &etas_host::HostSchema) -> Value {
    match schema {
        etas_host::HostSchema::Unit => json!({ "kind": "unit" }),
        etas_host::HostSchema::Bool => json!({ "kind": "bool" }),
        etas_host::HostSchema::Int => json!({ "kind": "int" }),
        etas_host::HostSchema::UInt => json!({ "kind": "uint" }),
        etas_host::HostSchema::Float => json!({ "kind": "float" }),
        etas_host::HostSchema::String => json!({ "kind": "string" }),
        etas_host::HostSchema::Bytes => json!({ "kind": "bytes" }),
        etas_host::HostSchema::Json => json!({ "kind": "json" }),
        etas_host::HostSchema::List(element) => {
            json!({ "kind": "list", "element": host_schema_snapshot(element) })
        }
        etas_host::HostSchema::Map { key, value } => json!({
            "kind": "map",
            "key": host_schema_snapshot(key),
            "value": host_schema_snapshot(value),
        }),
        etas_host::HostSchema::Record(fields) => json!({
            "kind": "record",
            "fields": fields.iter().map(|field| json!({
                "name": field.name,
                "schema": host_schema_snapshot(&field.schema),
                "optional": field.optional,
            })).collect::<Vec<_>>(),
        }),
        etas_host::HostSchema::Variant(variants) => json!({
            "kind": "variant",
            "variants": variants.iter().map(|variant| json!({
                "name": variant.name,
                "fields": variant.fields.iter().map(host_schema_snapshot).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        }),
    }
}

pub(crate) fn host_schema_from_snapshot(value: &Value) -> Result<etas_host::HostSchema, String> {
    match required_str(value, "kind")? {
        "unit" => Ok(etas_host::HostSchema::Unit),
        "bool" => Ok(etas_host::HostSchema::Bool),
        "int" => Ok(etas_host::HostSchema::Int),
        "uint" => Ok(etas_host::HostSchema::UInt),
        "float" => Ok(etas_host::HostSchema::Float),
        "string" => Ok(etas_host::HostSchema::String),
        "bytes" => Ok(etas_host::HostSchema::Bytes),
        "json" => Ok(etas_host::HostSchema::Json),
        "list" => Ok(etas_host::HostSchema::List(Box::new(
            host_schema_from_snapshot(required(value, "element")?)?,
        ))),
        "map" => Ok(etas_host::HostSchema::Map {
            key: Box::new(host_schema_from_snapshot(required(value, "key")?)?),
            value: Box::new(host_schema_from_snapshot(required(value, "value")?)?),
        }),
        "record" => Ok(etas_host::HostSchema::Record(
            required(value, "fields")?
                .as_array()
                .ok_or_else(|| "machine host record fields must be an array".to_owned())?
                .iter()
                .map(|field| {
                    Ok(etas_host::HostFieldSchema {
                        name: required_str(field, "name")?.to_owned(),
                        schema: host_schema_from_snapshot(required(field, "schema")?)?,
                        optional: required_bool(field, "optional")?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        )),
        "variant" => Ok(etas_host::HostSchema::Variant(
            required(value, "variants")?
                .as_array()
                .ok_or_else(|| "machine host variants must be an array".to_owned())?
                .iter()
                .map(|variant| {
                    Ok(etas_host::HostVariantSchema {
                        name: required_str(variant, "name")?.to_owned(),
                        fields: required(variant, "fields")?
                            .as_array()
                            .ok_or_else(|| {
                                "machine host variant fields must be an array".to_owned()
                            })?
                            .iter()
                            .map(host_schema_from_snapshot)
                            .collect::<Result<Vec<_>, _>>()?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        )),
        other => Err(format!("unknown machine host schema `{other}`")),
    }
}

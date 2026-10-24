use super::*;
use crate::orchestration::ActiveHandlerArmRecord;
use crate::value::{
    BytesValue, HostJsonSupportValue, MemoryWriteIntentValue, ModelResponseValue, NumericValue,
    PromptValue, ProvenanceValue, StringValue,
};

pub(super) enum ScalarValue {
    Unit,
    Bool(bool),
    Number(NumericValue),
    String(StringValue),
    Bytes(BytesValue),
    Json(HostJsonSupportValue),
    Prompt(PromptValue),
    Provenance(ProvenanceValue),
    ModelResponse(ModelResponseValue),
    Command {
        argv: Vec<String>,
        env: Vec<(String, String)>,
        cwd: Option<etas_host::WorkspacePathRef>,
        stdin: Option<Vec<u8>>,
    },
    CommandResult {
        exit_code: i32,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    MemoryWriteIntent(Box<MemoryWriteIntentValue>),
    OptionNone,
    Handler {
        fact_expr: HirExprId,
        handlers: Vec<ActiveHandlerArmRecord>,
    },
    ResourceHandle {
        name: String,
        stable_id: String,
        ty: TypeId,
    },
    WorkspacePath(etas_host::WorkspacePathRef),
    MemoryStore {
        region_stable_id: String,
        path: Vec<String>,
        key_type: TypeId,
        value_type: TypeId,
    },
}

pub(super) fn decode(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<ScalarValue, InterpreterCodecError> {
    match required_str(value, "kind")? {
        "unit" => Ok(ScalarValue::Unit),
        "bool" => Ok(ScalarValue::Bool(required_bool(value, "value")?)),
        "number" => Ok(ScalarValue::Number(numeric_value_from_json(value)?)),
        "string" => Ok(ScalarValue::String(
            required_str(value, "value")?.to_owned().into(),
        )),
        "bytes" => Ok(ScalarValue::Bytes(byte_array(value, "value")?.into())),
        "json" => Ok(ScalarValue::Json(host_json_support_value_from_json(
            required_obj(value, "value")?,
        )?)),
        "prompt" => Ok(ScalarValue::Prompt(
            required_array(value, "messages")?
                .iter()
                .map(|message| {
                    Ok(crate::value::PromptMessage {
                        role: value_codec::prompt_role_from_json(required_str(message, "role")?)
                            .map_err(InterpreterCodecError::new)?,
                        text: required_str(message, "text")?.into(),
                        trust: optional_string(message, "trust")?
                            .map(|wrapper| {
                                value_codec::trust_wrapper_from_json(&wrapper)
                                    .map_err(InterpreterCodecError::new)
                            })
                            .transpose()?,
                    })
                })
                .collect::<Result<crate::value::PromptValue, InterpreterCodecError>>()?,
        )),
        "provenance" => Ok(ScalarValue::Provenance(provenance_from_json(
            required_obj(value, "value")?,
        )?)),
        "model_response" => Ok(ScalarValue::ModelResponse(model_response_from_json(value)?)),
        "command" => Ok(ScalarValue::Command {
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
            cwd: match value.get("cwd") {
                Some(Value::Null) | None => None,
                Some(cwd) => Some(
                    etas_host::WorkspacePathRef::new(
                        etas_host::WorkspaceRegionId::new(required_str(cwd, "region")?.to_owned())
                            .map_err(|error| InterpreterCodecError::new(error.message))?,
                        required_str(cwd, "relative")?,
                    )
                    .map_err(|error| InterpreterCodecError::new(error.message))?,
                ),
            },
            stdin: match value.get("stdin") {
                Some(Value::Null) | None => None,
                Some(_) => Some(byte_array(value, "stdin")?),
            },
        }),
        "command_result" => Ok(ScalarValue::CommandResult {
            exit_code: required_i64(value, "exit_code").and_then(|exit_code| {
                i32::try_from(exit_code)
                    .map_err(|_| InterpreterCodecError::new("command exit_code must fit i32"))
            })?,
            stdout: byte_array(value, "stdout")?,
            stderr: byte_array(value, "stderr")?,
        }),
        "memory_write_intent" => {
            reject_unknown_fields(
                value,
                &["kind", "ty", "key_type", "value_type", "intent"],
                "memory write intent",
            )?;
            Ok(ScalarValue::MemoryWriteIntent(Box::new(
                crate::value::MemoryWriteIntentValue::restore(
                    etas_types::TypeId(required_u32(value, "ty")?),
                    etas_types::TypeId(required_u32(value, "key_type")?),
                    etas_types::TypeId(required_u32(value, "value_type")?),
                    required_str(value, "intent")?,
                    limits,
                )
                .map_err(InterpreterCodecError::new)?,
            )))
        }
        "option_none" => Ok(ScalarValue::OptionNone),
        "handler" => Ok(ScalarValue::Handler {
            fact_expr: HirExprId(required_u32(value, "fact_expr")?),
            handlers: required_array(value, "handlers")?
                .iter()
                .map(handler_arm_from_json)
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        }),
        "host_handle" => Err(InterpreterCodecError::new(
            "serialized host handles cannot be restored without a live host capability",
        )),
        "resource_handle" => Ok(ScalarValue::ResourceHandle {
            name: required_str(value, "name")?.to_owned(),
            stable_id: required_str(value, "stable_id")?.to_owned(),
            ty: etas_types::TypeId(required_u32(value, "ty")?),
        }),
        "workspace_path" => Ok(ScalarValue::WorkspacePath(
            etas_host::WorkspacePathRef::new(
                etas_host::WorkspaceRegionId::new(required_str(value, "region")?.to_owned())
                    .map_err(|error| InterpreterCodecError::new(error.message))?,
                required_str(value, "relative")?,
            )
            .map_err(|error| InterpreterCodecError::new(error.message))?,
        )),
        "memory_store" => Ok(ScalarValue::MemoryStore {
            region_stable_id: required_str(value, "region_stable_id")?.to_owned(),
            path: string_array(value, "path")?,
            key_type: etas_types::TypeId(required_u32(value, "key_type")?),
            value_type: etas_types::TypeId(required_u32(value, "value_type")?),
        }),
        other => Err(InterpreterCodecError::new(format!(
            "unsupported serialized interpreter value `{other}`"
        ))),
    }
}

impl From<ScalarValue> for InterpValue {
    fn from(value: ScalarValue) -> Self {
        match value {
            ScalarValue::Unit => Self::Unit,
            ScalarValue::OptionNone => Self::OptionNone,
            ScalarValue::Bool(value) => Self::Bool(value),
            ScalarValue::Number(value) => Self::Number(value),
            ScalarValue::String(value) => Self::String(value),
            ScalarValue::Bytes(value) => Self::Bytes(value),
            ScalarValue::Json(value) => Self::Json(value),
            ScalarValue::Prompt(value) => Self::Prompt(value),
            ScalarValue::Provenance(value) => Self::Provenance(value),
            ScalarValue::ModelResponse(value) => Self::ModelResponse(value),
            ScalarValue::MemoryWriteIntent(value) => Self::MemoryWriteIntent(value),
            ScalarValue::Command {
                argv,
                env,
                cwd,
                stdin,
            } => Self::Command {
                argv,
                env,
                cwd,
                stdin,
            },
            ScalarValue::CommandResult {
                exit_code,
                stdout,
                stderr,
            } => Self::CommandResult {
                exit_code,
                stdout,
                stderr,
            },
            ScalarValue::Handler {
                fact_expr,
                handlers,
            } => Self::Handler {
                fact_expr,
                handlers,
            },
            ScalarValue::ResourceHandle {
                name,
                stable_id,
                ty,
            } => Self::ResourceHandle {
                name,
                stable_id,
                ty,
            },
            ScalarValue::MemoryStore {
                region_stable_id,
                path,
                key_type,
                value_type,
            } => Self::MemoryStore {
                region_stable_id,
                path,
                key_type,
                value_type,
            },
            ScalarValue::WorkspacePath(value) => Self::WorkspacePath(value),
        }
    }
}
impl From<ScalarValue> for crate::orchestration::ValueSnapshot {
    fn from(value: ScalarValue) -> Self {
        match value {
            ScalarValue::Unit => Self::Unit,
            ScalarValue::OptionNone => Self::OptionNone,
            ScalarValue::Bool(value) => Self::Bool(value),
            ScalarValue::Number(value) => Self::Number(value),
            ScalarValue::String(value) => Self::String(value),
            ScalarValue::Bytes(value) => Self::Bytes(value),
            ScalarValue::Json(value) => Self::Json(value),
            ScalarValue::Prompt(value) => Self::Prompt(value),
            ScalarValue::Provenance(value) => Self::Provenance(value),
            ScalarValue::ModelResponse(value) => Self::ModelResponse(value),
            ScalarValue::MemoryWriteIntent(value) => Self::MemoryWriteIntent(value),
            ScalarValue::Command {
                argv,
                env,
                cwd,
                stdin,
            } => Self::Command {
                argv,
                env,
                cwd,
                stdin,
            },
            ScalarValue::CommandResult {
                exit_code,
                stdout,
                stderr,
            } => Self::CommandResult {
                exit_code,
                stdout,
                stderr,
            },
            ScalarValue::Handler {
                fact_expr,
                handlers,
            } => Self::Handler {
                fact_expr,
                handlers,
            },
            ScalarValue::ResourceHandle {
                name,
                stable_id,
                ty,
            } => Self::ResourceHandle {
                name,
                stable_id,
                ty,
            },
            ScalarValue::MemoryStore {
                region_stable_id,
                path,
                key_type,
                value_type,
            } => Self::MemoryStore {
                region_stable_id,
                path,
                key_type,
                value_type,
            },
            ScalarValue::WorkspacePath(value) => Self::WorkspacePath {
                region: value.region.as_str().to_owned(),
                relative: value.relative.to_string_lossy().into_owned(),
            },
        }
    }
}

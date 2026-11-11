use super::*;
use crate::orchestration::{
    ContinuationInputSnapshot as C, ControlSignalSnapshot as S, MachineInputSnapshot as I,
    TaskControlSnapshot as T,
};

pub(in crate::api::codec) fn encode(
    input: &I,
    budget: &mut snapshot::EncodingBudget,
) -> Result<Value, InterpreterCodecError> {
    budget.metadata(1, 0)?;
    Ok(match input {
        I::Signal(signal) => json!({"kind":"signal", "signal":signal_json(signal, budget)?}),
        I::EnterProgram {
            owner,
            call,
            args,
            span,
        } => json!({"kind":"enter_program", "owner":owner,
            "call":snapshot::call_target_json_with_budget(call, budget)?,
            "args":values_json(args, budget)?, "span":span_json(*span)}),
        I::SourceToolApproved => json!({"kind":"source_tool_approved"}),
        I::ModelResult(result) => {
            let value =
                json!({"kind":"model_result", "result":result_json(result, model_response_json)});
            budget.json_metadata(&value)?;
            value
        }
        I::ToolResult(result) => {
            let value = json!({"kind":"tool_result", "result":result_json(result, |response| json!({
                "id":response.id.0, "result":result_json(&response.result, value::host_value_json)
            }))});
            budget.json_metadata(&value)?;
            value
        }
    })
}

fn signal_json(
    signal: &S,
    budget: &mut snapshot::EncodingBudget,
) -> Result<Value, InterpreterCodecError> {
    budget.metadata(12, 0)?;
    Ok(match signal {
        S::Error {
            error_type,
            payload,
            span,
            continuation,
            residual,
        } => json!({
            "kind":"error", "type":error_type, "payload":snapshot::value_json_with_budget(payload, budget)?,
            "span":span_json(*span), "continuation":snapshot::continuation_json_with_budget(continuation, budget)?,
            "residual":residual,
        }),
        S::Input(input) => json!({"kind":"input", "input":continuation_input_json(input, budget)?}),
        S::Apply {
            continuation,
            input,
        } => json!({"kind":"apply",
            "continuation":snapshot::continuation_json_with_budget(continuation, budget)?,
            "input":continuation_input_json(input, budget)?}),
        S::Checkpoint {
            label,
            continuation,
        } => json!({"kind":"checkpoint", "label":label,
            "continuation":snapshot::continuation_json_with_budget(continuation, budget)?}),
        S::Expr {
            expr,
            frame,
            continuation,
        } => json!({"kind":"expr", "expr":expr.0,
            "frame":snapshot::frame_json_with_budget(frame, budget)?,
            "continuation":snapshot::continuation_json_with_budget(continuation, budget)?}),
        S::Block {
            agent_context,
            block,
            next_stmt_index,
            frame,
            continuation,
            residual,
        } => {
            json!({"kind":"block", "agent_context": machine::agent_context::request_json(agent_context.as_deref()),
            "block":block.0, "next_stmt_index":next_stmt_index, "residual":residual,
            "frame":snapshot::frame_json_with_budget(frame, budget)?,
            "continuation":snapshot::continuation_json_with_budget(continuation, budget)?})
        }
        S::Call {
            pure,
            site,
            target,
            args,
            span,
            continuation,
        } => json!({"kind":"call", "pure":pure,
            "site":site.as_ref().map(|site| snapshot::call_site_json_with_budget(site, budget)).transpose()?,
            "target":snapshot::call_target_json_with_budget(target, budget)?,
            "args":values_json(args, budget)?, "span":span_json(*span),
            "continuation":snapshot::continuation_json_with_budget(continuation, budget)?}),
        S::Task(task) => match task {
            T::Run { scope, span } => json!({"kind":"run", "scope":scope, "span":span_json(*span)}),
            T::Park {
                owner,
                task,
                token,
                span,
            } => json!({"kind":"park", "owner":owner, "task":task,
                "token":token, "span":span_json(*span)}),
            T::Requeue { token, value, span } => json!({"kind":"requeue", "token":token,
                "value":snapshot::value_json_with_budget(value, budget)?, "span":span_json(*span)}),
        },
        S::Fault(fault) => {
            let value = json!({"kind":"fault", "fault":fault});
            budget.json_metadata(&value)?;
            value
        }
    })
}

fn continuation_input_json(
    input: &C,
    budget: &mut snapshot::EncodingBudget,
) -> Result<Value, InterpreterCodecError> {
    let (kind, value) = match input {
        C::Value(value) => ("value", Some(value)),
        C::Return(value) => ("return", Some(value)),
        C::Resume(value) => ("resume", Some(value)),
        C::Finish(value) => ("finish", Some(value)),
        C::Break => ("break", None),
        C::Continue => ("continue", None),
    };
    Ok(
        json!({"kind":kind, "value":value.map(|value| snapshot::value_json_with_budget(value, budget)).transpose()?}),
    )
}

fn values_json(
    values: &[crate::orchestration::ValueSnapshot],
    budget: &mut snapshot::EncodingBudget,
) -> Result<Vec<Value>, InterpreterCodecError> {
    values
        .iter()
        .map(|value| snapshot::value_json_with_budget(value, budget))
        .collect()
}

pub(in crate::api::codec) fn decode(
    limits: &etas_host::StorageLimits,
    value: &Value,
    checked: &etas_frontend::CheckedProject,
) -> Result<I, InterpreterCodecError> {
    Ok(match required_str(value, "kind")? {
        "signal" => I::Signal(Box::new(signal_from_json(
            limits,
            required_obj(value, "signal")?,
            checked,
        )?)),
        "enter_program" => I::EnterProgram {
            owner: field(value, "owner")?,
            call: machine::call_target_snapshot_from_json(limits, required_obj(value, "call")?)
                .map_err(InterpreterCodecError::new)?,
            args: values_from_json(limits, value, "args")?,
            span: span_from_json(required_obj(value, "span")?)?,
        },
        "source_tool_approved" => I::SourceToolApproved,
        "model_result" => I::ModelResult(result_from_json(
            required_obj(value, "result")?,
            model_response_from_json,
        )?),
        "tool_result" => I::ToolResult(result_from_json(
            required_obj(value, "result")?,
            |response| {
                Ok(etas_host::ToolResponse {
                    id: HostRequestId(required_u32(response, "id")?),
                    result: result_from_json(
                        required_obj(response, "result")?,
                        value::host_value_from_json,
                    )?,
                })
            },
        )?),
        kind => {
            return Err(InterpreterCodecError::new(format!(
                "unknown checkpoint machine input `{kind}`"
            )));
        }
    })
}

fn signal_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
    checked: &etas_frontend::CheckedProject,
) -> Result<S, InterpreterCodecError> {
    let continuation = || {
        machine::continuation_from_snapshot(limits, required_obj(value, "continuation")?, checked)
            .map_err(InterpreterCodecError::new)
    };
    let frame = || {
        machine::locals_from_snapshot(limits, required_obj(value, "frame")?)
            .map_err(InterpreterCodecError::new)
    };
    let span = || span_from_json(required_obj(value, "span")?);
    Ok(match required_str(value, "kind")? {
        "input" => S::Input(continuation_input_from_json(
            limits,
            required_obj(value, "input")?,
        )?),
        "error" => S::Error {
            error_type: field(value, "type")?,
            payload: value::snapshot_from_json_with_limits(
                limits,
                required_obj(value, "payload")?,
            )?,
            span: span()?,
            continuation: continuation()?,
            residual: field(value, "residual")?,
        },
        "apply" => S::Apply {
            continuation: continuation()?,
            input: continuation_input_from_json(limits, required_obj(value, "input")?)?,
        },
        "checkpoint" => S::Checkpoint {
            label: required_optional_string(value, "label")?,
            continuation: continuation()?,
        },
        "expr" => S::Expr {
            expr: HirExprId(required_u32(value, "expr")?),
            frame: frame()?,
            continuation: continuation()?,
        },
        "block" => S::Block {
            agent_context: machine::agent_context::request_from_json(required_obj(
                value,
                "agent_context",
            )?)
            .map_err(InterpreterCodecError::new)?,
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            frame: frame()?,
            continuation: continuation()?,
            residual: field(value, "residual")?,
        },
        "call" => S::Call {
            pure: required_bool(value, "pure")?,
            site: match required_obj(value, "site")? {
                Value::Null => None,
                site => Some(
                    machine::call_site_from_json(limits, site)
                        .map_err(InterpreterCodecError::new)?,
                ),
            },
            target: machine::call_target_snapshot_from_json(limits, required_obj(value, "target")?)
                .map_err(InterpreterCodecError::new)?,
            args: values_from_json(limits, value, "args")?,
            span: span()?,
            continuation: continuation()?,
        },
        "run" => S::Task(T::Run {
            scope: required_u64(value, "scope")?,
            span: span()?,
        }),
        "park" => S::Task(T::Park {
            owner: required_u64(value, "owner")?,
            task: required_u64(value, "task")?,
            token: field(value, "token")?,
            span: span()?,
        }),
        "requeue" => S::Task(T::Requeue {
            token: field(value, "token")?,
            span: span()?,
            value: value::snapshot_from_json_with_limits(limits, required_obj(value, "value")?)?,
        }),
        "fault" => S::Fault(field(value, "fault")?),
        kind => {
            return Err(InterpreterCodecError::new(format!(
                "unknown checkpoint control input `{kind}`"
            )));
        }
    })
}

fn continuation_input_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<C, InterpreterCodecError> {
    let payload = || value::snapshot_from_json_with_limits(limits, required_obj(value, "value")?);
    Ok(match required_str(value, "kind")? {
        "value" => C::Value(payload()?),
        "return" => C::Return(payload()?),
        "resume" => C::Resume(payload()?),
        "finish" => C::Finish(payload()?),
        "break" => C::Break,
        "continue" => C::Continue,
        kind => {
            return Err(InterpreterCodecError::new(format!(
                "unknown continuation input `{kind}`"
            )));
        }
    })
}
fn values_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
    field: &'static str,
) -> Result<Vec<crate::orchestration::ValueSnapshot>, InterpreterCodecError> {
    required_array(value, field)?
        .iter()
        .map(|value| value::snapshot_from_json_with_limits(limits, value))
        .collect()
}
fn field<T: serde::de::DeserializeOwned>(
    value: &Value,
    field: &'static str,
) -> Result<T, InterpreterCodecError> {
    serde_json::from_value(required_obj(value, field)?.clone()).map_err(|error| {
        InterpreterCodecError::new(format!("invalid checkpoint input {field}: {error}"))
    })
}

fn result_json<T>(
    result: &Result<T, etas_host::HostError>,
    encode: impl FnOnce(&T) -> Value,
) -> Value {
    match result {
        Ok(value) => json!({"kind":"ok", "value":encode(value)}),
        Err(error) => json!({"kind":"error", "code":error.code.as_str(), "message":error.message,
            "details":error.details.iter().map(|detail| json!({"key":detail.key, "value":detail.value})).collect::<Vec<_>>()}),
    }
}
fn result_from_json<T>(
    value: &Value,
    decode: impl FnOnce(&Value) -> Result<T, InterpreterCodecError>,
) -> Result<Result<T, etas_host::HostError>, InterpreterCodecError> {
    if required_str(value, "kind")? == "ok" {
        return decode(required_obj(value, "value")?).map(Ok);
    }
    if required_str(value, "kind")? != "error" {
        return Err(InterpreterCodecError::new("invalid Host result kind"));
    }
    use etas_host::HostErrorCode as E;
    let code = match required_str(value, "code")? {
        "ProviderRejected" => E::ProviderRejected,
        "ProviderUnavailable" => E::ProviderUnavailable,
        "ToolRejected" => E::ToolRejected,
        "ToolUnavailable" => E::ToolUnavailable,
        "InvalidRequest" => E::InvalidRequest,
        "InvalidResponse" => E::InvalidResponse,
        "IncompleteResponse" => E::IncompleteResponse,
        "IoFailure" => E::IoFailure,
        "SchemaMismatch" => E::SchemaMismatch,
        "BudgetExceeded" => E::BudgetExceeded,
        "AuthorityDenied" => E::AuthorityDenied,
        "TimedOut" => E::TimedOut,
        "Cancelled" => E::Cancelled,
        "Closed" => E::Closed,
        "Interrupted" => E::Interrupted,
        _ => {
            return Err(InterpreterCodecError::new(
                "invalid checkpoint Host error code",
            ));
        }
    };
    let mut error = etas_host::HostError::new(code, required_str(value, "message")?);
    for detail in required_array(value, "details")? {
        error = error.with_detail(required_str(detail, "key")?, required_str(detail, "value")?);
    }
    Ok(Err(error))
}

fn model_response_json(response: &etas_host::ModelResponse) -> Value {
    json!({"id":response.id.0, "usage":response.usage.as_ref().map(value::model_usage::encode),
    "result":result_json(&response.result, |output| json!({
        "message":model_host_message_json(&output.message),
        "provider_request_id":output.provider_request_id,
        "termination": match &output.termination {
            etas_host::ModelTermination::Complete => json!({"kind":"complete"}),
            etas_host::ModelTermination::Truncated => json!({"kind":"truncated"}),
            etas_host::ModelTermination::Refused(reason) => json!({"kind":"refused","reason":reason}),
            etas_host::ModelTermination::InvalidToolCall(reason) => json!({"kind":"invalid_tool_call","reason":reason}),
            etas_host::ModelTermination::InvalidOutput(reason) => json!({"kind":"invalid_output","reason":reason}),
        },
    }))})
}
fn model_response_from_json(
    value: &Value,
) -> Result<etas_host::ModelResponse, InterpreterCodecError> {
    Ok(etas_host::ModelResponse {
        id: HostRequestId(required_u32(value, "id")?),
        usage: match required_obj(value, "usage")? {
            Value::Null => None,
            usage => Some(value::model_usage::decode(usage)?),
        },
        result: result_from_json(required_obj(value, "result")?, |output| {
            Ok(etas_host::ModelOutput {
                message: model_host_message_from_json(required_obj(output, "message")?)?,
                provider_request_id: match required_obj(output, "provider_request_id")? {
                    Value::Null => None,
                    _ => Some(required_str(output, "provider_request_id")?.into()),
                },
                termination: {
                    let termination = required_obj(output, "termination")?;
                    match required_str(termination, "kind")? {
                        "complete" => etas_host::ModelTermination::Complete,
                        "truncated" => etas_host::ModelTermination::Truncated,
                        "refused" => etas_host::ModelTermination::Refused(
                            required_str(termination, "reason")?.into(),
                        ),
                        "invalid_tool_call" => etas_host::ModelTermination::InvalidToolCall(
                            required_str(termination, "reason")?.into(),
                        ),
                        "invalid_output" => etas_host::ModelTermination::InvalidOutput(
                            required_str(termination, "reason")?.into(),
                        ),
                        _ => return Err(InterpreterCodecError::new("invalid model termination")),
                    }
                },
            })
        })?,
    })
}

#[cfg(test)]
pub(crate) fn roundtrip(input: &I, checked: &etas_frontend::CheckedProject) -> I {
    let value = encode(input, &mut snapshot::EncodingBudget::default()).unwrap();
    let bytes = serde_json::to_vec(&value).unwrap();
    decode(
        &etas_host::StorageLimits::default(),
        &serde_json::from_slice(&bytes).unwrap(),
        checked,
    )
    .unwrap()
}

#[cfg(test)]
pub(crate) fn roundtrip_machine(
    machine: &MachineSnapshot,
    checked: &etas_frontend::CheckedProject,
) -> MachineSnapshot {
    let value = super::machine_json(machine).unwrap();
    let bytes = serde_json::to_vec(&value).unwrap();
    let value = serde_json::from_slice(&bytes).unwrap();
    super::machine_from_json_with_revisions(
        &etas_host::StorageLimits::default(),
        &value,
        checked,
        None,
    )
    .unwrap()
}

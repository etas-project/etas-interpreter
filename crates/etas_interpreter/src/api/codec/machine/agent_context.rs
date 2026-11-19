use crate::orchestration::{AgentExecutionStateSnapshot, AgentRequestContextSnapshot};
use serde_json::{Value, json};

pub(in crate::api::codec) fn request_json(request: Option<&AgentRequestContextSnapshot>) -> Value {
    match request {
        None => Value::Null,
        Some(request) => json!({
            "program": request.program, "method": request.method.0, "request": request.request.0,
            "policy": super::model_policy_snapshot(&request.policy),
        }),
    }
}

pub(in crate::api::codec) fn request_from_json(
    value: &Value,
) -> Result<Option<Box<AgentRequestContextSnapshot>>, String> {
    use super::value::{required, required_u32};
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(Box::new(AgentRequestContextSnapshot {
        program: serde_json::from_value(required(value, "program")?.clone())
            .map_err(|error| format!("invalid agent context program: {error}"))?,
        method: etas_hir::SymbolId(required_u32(value, "method")?),
        request: etas_host::HostRequestId(required_u32(value, "request")?),
        policy: super::model_policy_from_snapshot(required(value, "policy")?)?,
    })))
}

pub(in crate::api::codec) fn state_json(state: &AgentExecutionStateSnapshot) -> Value {
    json!({ "request": request_json(state.request.as_deref()), "policy": super::model_policy_snapshot(&state.policy) })
}

pub(in crate::api::codec) fn state_from_json(
    value: &Value,
) -> Result<AgentExecutionStateSnapshot, String> {
    use super::value::required;
    Ok(AgentExecutionStateSnapshot {
        request: request_from_json(required(value, "request")?)?,
        policy: super::model_policy_from_snapshot(required(value, "policy")?)?,
    })
}

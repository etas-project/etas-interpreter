use super::{protocol, validate};
use crate::value::{ArrayValue, ConversationValue, InterpValue, PromptValue, SharedValue};
use etas_host::StorageLimits;

fn resource(message: String) -> protocol::ProtocolError {
    protocol::ProtocolError {
        code: "resource_limit",
        message,
    }
}

pub(crate) fn from_prompt(
    prompt: PromptValue,
    limits: &StorageLimits,
) -> Result<ConversationValue, protocol::ProtocolError> {
    let value = ConversationValue {
        protocol: Some(protocol::from_prompt(prompt)),
        session: None,
        selected_context: None,
        history_fence: None,
        messages: Vec::new().into(),
        cursor: None,
    };
    validate(&value, limits).map_err(resource)?;
    Ok(value)
}

fn update(
    mut value: ConversationValue,
    limits: &StorageLimits,
    apply: impl FnOnce(&SharedValue) -> Result<SharedValue, protocol::ProtocolError>,
) -> Result<ConversationValue, protocol::ProtocolError> {
    validate(&value, limits).map_err(resource)?;
    let state = value
        .protocol
        .as_ref()
        .cloned()
        .unwrap_or_else(|| protocol::from_prompt(Default::default()));
    value.protocol = Some(apply(&state)?);
    validate(&value, limits).map_err(resource)?;
    Ok(value)
}

pub(crate) fn append_step(
    value: ConversationValue,
    step: &InterpValue,
    limits: &StorageLimits,
) -> Result<ConversationValue, protocol::ProtocolError> {
    update(value, limits, |state| {
        protocol::append_step(state, step, limits.max_scan_rows)
    })
}

pub(crate) fn append_results(
    value: ConversationValue,
    results: &ArrayValue,
    limits: &StorageLimits,
) -> Result<ConversationValue, protocol::ProtocolError> {
    update(value, limits, |state| {
        protocol::append_results(state, results, limits.max_scan_rows)
    })
}

pub(crate) fn append_prompt(
    value: ConversationValue,
    prompt: PromptValue,
    limits: &StorageLimits,
) -> Result<ConversationValue, protocol::ProtocolError> {
    update(value, limits, |state| {
        protocol::append_prompt(state, prompt, limits.max_scan_rows)
    })
}

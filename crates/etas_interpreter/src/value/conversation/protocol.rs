use std::collections::{BTreeMap, BTreeSet};

use crate::value::{
    ArrayValue, InterpValue as V, ListValue, PromptMessage, PromptValue, RecordValue, SharedValue,
};

#[derive(Debug)]
pub(crate) struct ProtocolError {
    pub code: &'static str,
    pub message: String,
}

fn invalid(message: impl Into<String>) -> ProtocolError {
    ProtocolError {
        code: "invalid_protocol",
        message: message.into(),
    }
}

pub(crate) fn field<'a>(value: &'a V, name: &str) -> Result<&'a V, ProtocolError> {
    let value = match value {
        V::Nominal { value, .. } => value.as_ref(),
        value => value,
    };
    let V::Record(record) = value else {
        return Err(invalid("protocol value is not a record"));
    };
    record
        .get_ref(name)
        .ok_or_else(|| invalid(format!("protocol record has no `{name}` field")))
}

fn string_field<'a>(value: &'a V, name: &str) -> Result<&'a str, ProtocolError> {
    match field(value, name)? {
        V::String(value) => Ok(value.as_str()),
        _ => Err(invalid(format!("protocol `{name}` is not text"))),
    }
}

pub(crate) fn from_prompt(prompt: PromptValue) -> SharedValue {
    V::Record(RecordValue::new(vec![
        ("prompt".into(), V::Prompt(prompt)),
        ("turns".into(), V::List(ListValue::default())),
    ]))
    .into()
}

pub(crate) fn parts(state: &V) -> Result<(&[PromptMessage], &ListValue), ProtocolError> {
    let V::Prompt(prompt) = field(state, "prompt")? else {
        return Err(invalid("conversation has no Prompt"));
    };
    let V::List(turns) = field(state, "turns")? else {
        return Err(invalid("conversation has no protocol turn log"));
    };
    Ok((prompt, turns))
}

fn call_ids(calls: &ArrayValue) -> Result<Vec<&str>, ProtocolError> {
    let mut seen = BTreeSet::new();
    let mut ids = Vec::with_capacity(calls.borrow().len());
    for call in calls.borrow().iter() {
        let id = string_field(call, "id")?;
        if id.is_empty() || !seen.insert(id) {
            return Err(invalid(
                "tool call IDs must be nonempty and unique within a turn",
            ));
        }
        ids.push(id);
    }
    Ok(ids)
}

fn correlate<'a>(calls: &ArrayValue, results: &'a ArrayValue) -> Result<Vec<&'a V>, ProtocolError> {
    let ids = call_ids(calls)?;
    let mut by_id = BTreeMap::new();
    for result in results.borrow().iter() {
        let id = string_field(result, "call_id")?;
        if by_id.insert(id, result).is_some() {
            return Err(invalid("tool results contain duplicate call IDs"));
        }
    }
    if ids.len() != by_id.len() {
        return Err(invalid(
            "tool results must complete exactly the pending call set",
        ));
    }
    ids.into_iter()
        .map(|id| {
            by_id
                .remove(id)
                .ok_or_else(|| invalid("tool results contain a missing or foreign call ID"))
        })
        .collect()
}

pub(crate) fn validate(state: &V, limit: usize) -> Result<Option<&ArrayValue>, ProtocolError> {
    let (prompt, turns) = parts(state)?;
    if prompt.len() > limit || turns.len() > limit {
        return Err(ProtocolError {
            code: "resource_limit",
            message: "conversation exceeds its turn/content limit".into(),
        });
    }
    let chronological = turns.iter().collect::<Vec<_>>();
    let mut pending = None;
    for event in chronological.into_iter().rev() {
        match event {
            V::Record(_) => {
                if pending.is_some() {
                    return Err(invalid("append tool results before another assistant turn"));
                }
                let V::Array(calls) = field(event, "calls")? else {
                    return Err(invalid("assistant turn has no tool call array"));
                };
                field(event, "turn")?;
                call_ids(calls)?;
                pending = (!calls.borrow().is_empty()).then_some(calls);
            }
            V::Array(results) => {
                let calls = pending
                    .take()
                    .ok_or_else(|| invalid("tool results have no pending assistant requests"))?;
                correlate(calls, results)?;
            }
            V::Prompt(prompt) => {
                if pending.is_some() || prompt.len() > limit {
                    return Err(invalid(
                        "prompt instruction cannot interrupt pending tool results",
                    ));
                }
            }
            _ => return Err(invalid("unknown conversation protocol event")),
        }
    }
    Ok(pending)
}

fn append(state: &SharedValue, event: V) -> Result<SharedValue, ProtocolError> {
    let (_, turns) = parts(state)?;
    let mut turns = turns.clone();
    turns.push_front(event);
    let mut next = state.clone();
    let V::Record(record) = next.make_mut() else {
        return Err(invalid("conversation state is not a record"));
    };
    *record
        .field_mut("turns")
        .ok_or_else(|| invalid("conversation turn log is absent"))? = V::List(turns);
    Ok(next)
}

pub(crate) fn append_prompt(
    state: &SharedValue,
    prompt: PromptValue,
    limit: usize,
) -> Result<SharedValue, ProtocolError> {
    if validate(state, limit)?.is_some() {
        return Err(invalid("append tool results before a prompt instruction"));
    }
    append(state, V::Prompt(prompt))
}

pub(crate) fn append_step(
    state: &SharedValue,
    step: &V,
    limit: usize,
) -> Result<SharedValue, ProtocolError> {
    if validate(state, limit)?.is_some() {
        return Err(invalid(
            "append all pending tool results before another assistant turn",
        ));
    }
    let decision = field(step, "decision")?;
    let decision = match decision {
        V::Nominal { value, .. } => value.as_ref(),
        value => value,
    };
    let V::Variant { name, fields } = decision else {
        return Err(invalid("step decision is not an enum variant"));
    };
    let calls = match (name.as_str(), fields.as_slice()) {
        ("Final" | "Stopped", [_]) => ArrayValue::new(Vec::new()),
        ("CallTools", [V::Array(calls)]) if !calls.borrow().is_empty() => {
            call_ids(calls)?;
            calls.clone()
        }
        _ => {
            return Err(invalid(
                "step decision has an invalid payload or empty tool request list",
            ));
        }
    };
    let next = append(
        state,
        V::Record(RecordValue::new(vec![
            ("turn".into(), field(step, "turn")?.clone()),
            ("calls".into(), V::Array(calls)),
        ])),
    )?;
    validate(&next, limit)?;
    Ok(next)
}

pub(crate) fn append_results(
    state: &SharedValue,
    results: &ArrayValue,
    limit: usize,
) -> Result<SharedValue, ProtocolError> {
    let calls = validate(state, limit)?
        .ok_or_else(|| invalid("conversation has no pending tool results"))?;
    let ordered = correlate(calls, results)?.into_iter().cloned().collect();
    let next = append(state, V::Array(ArrayValue::new(ordered)))?;
    validate(&next, limit)?;
    Ok(next)
}

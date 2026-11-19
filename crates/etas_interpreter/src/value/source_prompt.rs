//! Convert the checked source prompt only at the model boundary. Handler arms
//! and ordinary Etas methods keep their original nominal record values.
use super::{InterpValue as V, PromptMessage, PromptRole, PromptValue, RecordValue, StringValue};
use etas_types::{TrustWrapper, TypeId};

pub(crate) fn trusted_literal(ty: TypeId, literal: StringValue) -> V {
    let content = V::String(literal);
    let evidence = V::Trust {
        wrapper: TrustWrapper::Trusted,
        value: content.clone().into(),
    };
    V::Nominal {
        ty,
        value: V::Record(vec![("value".into(), content), ("evidence".into(), evidence)].into())
            .into(),
    }
}

pub(crate) fn evidence_type(program: &etas_frontend::CheckedProject, expected: TypeId) -> bool {
    use etas_types::Type;
    let Some(Type::Nominal(trusted)) =
        crate::eval::resolve_std_type(program, &["std", "security", "trust", "Trusted"])
            .and_then(|trusted| program.type_store.get(trusted))
    else {
        return false;
    };
    let Some(Type::Record(fields)) = trusted
        .representation
        .and_then(|repr| program.type_store.get(repr))
    else {
        return false;
    };
    fields
        .fields
        .iter()
        .any(|field| field.name == "evidence" && field.ty == expected)
}

fn record(value: &V) -> Result<&RecordValue, String> {
    if let V::Nominal { value, .. } = value
        && let V::Record(record) = value.as_ref()
    {
        return Ok(record);
    }
    Err("source prompt channel requires a checked nominal record".into())
}

fn text(value: Option<&V>) -> Result<&StringValue, String> {
    match value {
        Some(V::String(text)) => Ok(text),
        _ => Err("source prompt channel has no string content".into()),
    }
}

fn message(value: &V) -> Result<PromptMessage, String> {
    let V::Nominal { value, .. } = value else {
        return Err("source prompt contains a non-channel value".into());
    };
    let V::Variant { name, fields } = value.as_ref() else {
        return Err("source prompt has an invalid channel".into());
    };
    let [content] = fields.as_slice() else {
        return Err("source prompt channel requires one payload".into());
    };
    let (role, text, trust) = match name.as_str() {
        "System" => {
            let trusted = record(content)?;
            let payload = text(trusted.get_ref("value"))?;
            let Some(V::Trust {
                wrapper: TrustWrapper::Trusted,
                value,
            }) = trusted.get_ref("evidence")
            else {
                return Err("system prompt requires trusted instruction evidence".into());
            };
            if !matches!(value.as_ref(), V::String(original) if original == payload) {
                return Err("system prompt text differs from its trusted literal evidence".into());
            }
            (PromptRole::System, payload, Some(TrustWrapper::Trusted))
        }
        "User" => (
            PromptRole::User,
            text(record(content)?.get_ref("value"))?,
            Some(TrustWrapper::Public),
        ),
        "Assistant" => (PromptRole::Assistant, text(Some(content))?, None),
        "Data" => (
            PromptRole::Data,
            text(record(content)?.get_ref("text"))?,
            None,
        ),
        _ => return Err("unknown source prompt channel".into()),
    };
    Ok(PromptMessage {
        role,
        text: text.clone(),
        trust,
    })
}

pub(crate) fn to_host(value: &V, expected: TypeId) -> Result<PromptValue, String> {
    if !matches!(value, V::Nominal {ty, ..} if *ty == expected) {
        return Err("inference did not receive its checked source Prompt".into());
    }
    let Some(V::Array(parts)) = record(value)?.get_ref("parts") else {
        return Err("source Prompt requires its checked channel array".into());
    };
    parts.borrow().iter().map(message).collect()
}

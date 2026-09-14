use super::*;
use crate::value::ConversationValue;

pub(in crate::api::codec) fn selected_context_json(
    context: &etas_host::session::SessionPublishedContext,
) -> Value {
    json!({"text":context.content.text,"provenance":context.content.provenance,
        "fence":context.fence.as_token(),"version":context.version})
}

fn selected_context_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<etas_host::session::SessionPublishedContext, InterpreterCodecError> {
    reject_unknown_fields(
        value,
        &["text", "provenance", "fence", "version"],
        "published session context",
    )?;
    let provenance = value
        .get("provenance")
        .and_then(Value::as_object)
        .ok_or_else(|| InterpreterCodecError::new("missing or invalid context provenance"))?;
    if provenance.len() > limits.max_nodes {
        return Err(InterpreterCodecError::new(
            "context provenance exceeds node limit",
        ));
    }
    let text = required_str(value, "text")?;
    let fence = required_str(value, "fence")?;
    let mut bytes = text.len().checked_add(fence.len());
    for (key, value) in provenance {
        let value = value.as_str().ok_or_else(|| {
            InterpreterCodecError::new("context provenance values must be strings")
        })?;
        bytes = bytes
            .and_then(|n| n.checked_add(key.len()))
            .and_then(|n| n.checked_add(value.len()));
    }
    if bytes.is_none_or(|bytes| bytes > limits.max_value_bytes) {
        return Err(InterpreterCodecError::new(
            "published context exceeds storage limits",
        ));
    }
    let provenance = provenance
        .iter()
        .map(|(k, v)| {
            v.as_str()
                .map(|v| (k.clone(), v.to_owned()))
                .ok_or_else(|| {
                    InterpreterCodecError::new("context provenance values must be strings")
                })
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
    let context = etas_host::session::SessionPublishedContext {
        content: etas_host::session::SessionContextContent {
            text: text.to_owned(),
            provenance,
        },
        fence: etas_host::session::SessionHistoryFence::from_token(fence.to_owned(), limits)
            .map_err(|e| InterpreterCodecError::new(e.message))?,
        version: required_u64(value, "version")?,
    };
    context
        .storage_size(limits)
        .map_err(|e| InterpreterCodecError::new(e.message))?;
    Ok(context)
}

pub(super) struct MessageParts<T> {
    id: String,
    from: Option<String>,
    to: Option<String>,
    role: crate::value::MessageRoleValue,
    session: Option<String>,
    created_at: String,
    payload: T,
    provenance: Option<crate::value::ProvenanceValue>,
}

pub(super) fn message_parts<T>(
    limits: &etas_host::StorageLimits,
    value: &Value,
    mut decode_payload: impl FnMut(
        &etas_host::StorageLimits,
        &Value,
    ) -> Result<T, InterpreterCodecError>,
) -> Result<MessageParts<T>, InterpreterCodecError> {
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
    Ok(MessageParts {
        id: required_str(value, "id")?.to_owned(),
        from: required_optional_string(value, "from")?,
        to: required_optional_string(value, "to")?,
        role: value_codec::message_role_from_json(required_str(value, "role")?)
            .map_err(InterpreterCodecError::new)?,
        session: required_optional_string(value, "session")?,
        created_at: required_str(value, "created_at")?.to_owned(),
        payload: decode_payload(limits, required_obj(value, "payload")?)?,
        provenance: if provenance.is_null() {
            None
        } else {
            Some(provenance_from_json(provenance)?)
        },
    })
}

impl From<MessageParts<InterpValue>> for crate::value::MessageValue {
    fn from(parts: MessageParts<InterpValue>) -> Self {
        Self {
            id: parts.id,
            from: parts.from,
            to: parts.to,
            role: parts.role,
            session: parts.session,
            created_at: parts.created_at,
            payload: Box::new(parts.payload),
            provenance: parts.provenance,
        }
    }
}

impl From<MessageParts<crate::orchestration::ValueSnapshot>>
    for crate::orchestration::MessageSnapshot
{
    fn from(parts: MessageParts<crate::orchestration::ValueSnapshot>) -> Self {
        Self {
            id: parts.id,
            from: parts.from,
            to: parts.to,
            role: parts.role,
            session: parts.session,
            created_at: parts.created_at,
            payload: crate::orchestration::SnapshotBox::new(parts.payload),
            provenance: parts.provenance,
        }
    }
}

struct ConversationParts<M> {
    selected_context: Option<etas_host::session::SessionPublishedContext>,
    history_fence: Option<etas_host::session::SessionHistoryFence>,
    session: String,
    messages: Vec<M>,
    cursor: Option<String>,
}

fn conversation_parts<M>(
    limits: &etas_host::StorageLimits,
    value: &Value,
    mut decode_message: impl FnMut(
        &etas_host::StorageLimits,
        &Value,
    ) -> Result<M, InterpreterCodecError>,
) -> Result<ConversationParts<M>, InterpreterCodecError> {
    crate::value::conversation::validate_json(value, limits).map_err(InterpreterCodecError::new)?;
    reject_unknown_fields(
        value,
        &[
            "kind",
            "session",
            "messages",
            "cursor",
            "history_fence",
            "selected_context",
        ],
        "conversation value",
    )?;
    let messages = required_array(value, "messages")?
        .iter()
        .map(|message| {
            if required_str(message, "kind")? != "message" {
                return Err(InterpreterCodecError::new(
                    "conversation entries must be message values",
                ));
            }
            decode_message(limits, message)
        })
        .collect::<Result<Vec<_>, InterpreterCodecError>>()?;
    let conversation = ConversationParts {
        selected_context: match value.get("selected_context") {
            Some(Value::Null) => None,
            Some(value) => Some(selected_context_from_json(limits, value)?),
            None => return Err(InterpreterCodecError::new("missing `selected_context`")),
        },
        history_fence: required_optional_string(value, "history_fence")?
            .map(|token| {
                etas_host::session::SessionHistoryFence::from_token(token, limits)
                    .map_err(|error| InterpreterCodecError::new(error.message))
            })
            .transpose()?,
        session: required_str(value, "session")?.to_owned(),
        messages,
        cursor: required_optional_string(value, "cursor")?,
    };
    Ok(conversation)
}
pub(super) fn conversation_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<ConversationValue, InterpreterCodecError> {
    let parts = conversation_parts(limits, value, |limits, value| {
        message_parts(limits, value, value_from_json_with_limits).map(Into::into)
    })?;
    let conversation = ConversationValue {
        selected_context: parts.selected_context.map(Box::new),
        history_fence: parts.history_fence,
        session: parts.session,
        messages: parts.messages,
        cursor: parts.cursor,
    };
    crate::value::conversation::validate(&conversation, limits)
        .map_err(InterpreterCodecError::new)?;
    Ok(conversation)
}

pub(super) fn conversation_snapshot_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<crate::orchestration::ConversationSnapshot, InterpreterCodecError> {
    let parts = conversation_parts(limits, value, |limits, value| {
        message_parts(limits, value, snapshot_from_json_with_limits).map(Into::into)
    })?;
    Ok(crate::orchestration::ConversationSnapshot {
        selected_context: parts.selected_context,
        history_fence: parts.history_fence,
        session: parts.session,
        messages: parts.messages,
        cursor: parts.cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation() -> InterpValue {
        InterpValue::Conversation(ConversationValue {
            selected_context: None,
            session: "session".into(),
            history_fence: None,
            messages: vec![crate::value::MessageValue {
                id: "one".into(),
                from: Some("sender".into()),
                to: None,
                role: crate::value::MessageRoleValue::User,
                session: Some("session".into()),
                created_at: "42".into(),
                payload: Box::new(InterpValue::Nominal {
                    ty: etas_types::TypeId(27),
                    value: crate::value::SharedValue::new(InterpValue::String("payload".into())),
                }),
                provenance: Some(crate::value::ProvenanceValue {
                    trace_id: Some("trace".into()),
                    source: None,
                }),
            }],
            cursor: Some("opaque cursor".into()),
        })
    }

    #[test]
    fn conversation_codec_preserves_cursor_and_nominal_message_payload() {
        let expected = conversation();
        assert_eq!(value_from_json(&value_json(&expected)).unwrap(), expected);
        let empty = InterpValue::Conversation(ConversationValue {
            selected_context: None,
            session: "empty".into(),
            history_fence: None,
            messages: Vec::new(),
            cursor: None,
        });
        assert_eq!(value_from_json(&value_json(&empty)).unwrap(), empty);
    }

    #[test]
    fn conversation_codec_rejects_missing_or_malformed_fields() {
        let original = value_json(&conversation());
        for key in [
            "session",
            "messages",
            "cursor",
            "history_fence",
            "selected_context",
        ] {
            let mut invalid = original.clone();
            invalid.as_object_mut().unwrap().remove(key);
            assert!(value_from_json(&invalid).is_err(), "missing {key}");
        }
        let mut invalid = original.clone();
        invalid["messages"][0] = json!({"kind":"unit"});
        assert!(value_from_json(&invalid).is_err());
        let mut invalid = original;
        invalid["extra"] = json!(true);
        assert!(value_from_json(&invalid).is_err());
    }
}

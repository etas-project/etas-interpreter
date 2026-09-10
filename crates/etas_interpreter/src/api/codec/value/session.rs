use super::*;
use crate::value::ConversationValue;

pub(super) fn selected_context_json(
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

pub(super) fn conversation_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<ConversationValue, InterpreterCodecError> {
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
            match value_from_json_with_limits(limits, message)? {
                InterpValue::Message(message) => Ok(message),
                _ => Err(InterpreterCodecError::new(
                    "conversation message decoder returned another value kind",
                )),
            }
        })
        .collect::<Result<Vec<_>, InterpreterCodecError>>()?;
    let conversation = ConversationValue {
        selected_context: match value.get("selected_context") {
            Some(Value::Null) => None,
            Some(value) => Some(Box::new(selected_context_from_json(limits, value)?)),
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
    crate::value::conversation::validate(&conversation, limits)
        .map_err(InterpreterCodecError::new)?;
    Ok(conversation)
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
                    value: Box::new(InterpValue::String("payload".into())),
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

use std::collections::BTreeMap;

use super::*;
use crate::value::{ConversationValue, MessageRoleValue, MessageValue, ProvenanceValue};

// These field views serve both owned HostValue and streaming JSON sinks.
enum Field<'a> {
    Text(&'a str),
    NullableText(Option<&'a str>),
    OptionalText(Option<&'a str>),
    Value(&'a InterpValue),
    Message(&'a MessageValue),
    Messages(&'a [MessageValue]),
    OptionalProvenance(Option<&'a ProvenanceValue>),
    Provenance(&'a ProvenanceValue),
    PublishedContext(Option<&'a etas_host::session::SessionPublishedContext>),
    StringFields(&'a BTreeMap<String, String>),
    Version(u64),
}

pub(super) fn message<V: HostValueVisitor>(
    message: &MessageValue,
    visitor: V,
) -> Result<V::Output, HostError> {
    use Field::*;
    let role = match message.role {
        MessageRoleValue::System => "system",
        MessageRoleValue::User => "user",
        MessageRoleValue::Assistant => "assistant",
        MessageRoleValue::Tool => "tool",
    };
    visitor.record([
        ("id", Text(&message.id)),
        ("from", OptionalText(message.from.as_deref())),
        ("to", OptionalText(message.to.as_deref())),
        ("role", Text(role)),
        ("session", OptionalText(message.session.as_deref())),
        ("created_at", Text(&message.created_at)),
        ("payload", Value(&message.payload)),
        (
            "provenance",
            OptionalProvenance(message.provenance.as_ref()),
        ),
    ])
}

pub(super) fn conversation<V: HostValueVisitor>(
    conversation: &ConversationValue,
    visitor: V,
) -> Result<V::Output, HostError> {
    use Field::*;
    visitor.record([
        (
            "selected_context",
            PublishedContext(conversation.selected_context.as_deref()),
        ),
        (
            "history_fence",
            NullableText(conversation.history_fence.as_ref().map(|f| f.as_token())),
        ),
        ("session", Text(&conversation.session)),
        ("messages", Messages(&conversation.messages)),
        ("cursor", NullableText(conversation.cursor.as_deref())),
    ])
}

impl HostValueProjection for Field<'_> {
    fn project<V: HostValueVisitor>(&self, visitor: V) -> Result<V::Output, HostError> {
        match self {
            Self::Text(text) => visitor.scalar(HostScalar::String(text)),
            Self::NullableText(text) => visitor.scalar(match text {
                Some(text) => HostScalar::String(text),
                None => HostScalar::Unit,
            }),
            Self::OptionalText(text) => visitor.variant(
                if text.is_some() { "Some" } else { "None" },
                text.map(Self::Text),
            ),
            Self::OptionalProvenance(value) => visitor.variant(
                if value.is_some() { "Some" } else { "None" },
                value.map(Self::Provenance),
            ),
            Self::Value(value) => BorrowedValue(value).project(visitor),
            Self::Message(value) => message(value, visitor),
            Self::Messages(values) => visitor.list(values.iter().map(Self::Message)),
            Self::Provenance(value) => visitor.record([
                ("trace_id", Self::NullableText(value.trace_id.as_deref())),
                ("source", Self::NullableText(value.source.as_deref())),
            ]),
            Self::PublishedContext(None) => visitor.scalar(HostScalar::Unit),
            Self::PublishedContext(Some(value)) => visitor.record([
                ("text", Self::Text(&value.content.text)),
                ("provenance", Self::StringFields(&value.content.provenance)),
                ("fence", Self::Text(value.fence.as_token())),
                ("version", Self::Version(value.version)),
            ]),
            Self::StringFields(fields) => {
                visitor.record(fields.iter().map(|(k, v)| (k.as_str(), Self::Text(v))))
            }
            Self::Version(value) => visitor.scalar(HostScalar::UInt(*value as u128)),
        }
    }
}

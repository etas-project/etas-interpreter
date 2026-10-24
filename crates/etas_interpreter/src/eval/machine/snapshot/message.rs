use crate::{
    orchestration::{ConversationSnapshot, MessageSnapshot, SnapshotBox, ValueSnapshot},
    value::{
        ConversationValue, InterpValue, MessageList, MessageRoleValue, MessageValue,
        ProvenanceValue, SharedValue,
    },
};

pub(super) struct MessageHeader {
    id: String,
    from: Option<String>,
    to: Option<String>,
    role: MessageRoleValue,
    session: Option<String>,
    created_at: String,
    provenance: Option<ProvenanceValue>,
}

impl MessageHeader {
    pub(super) fn capture(message: &MessageValue) -> Self {
        Self {
            id: message.id.clone(),
            from: message.from.clone(),
            to: message.to.clone(),
            role: message.role,
            session: message.session.clone(),
            created_at: message.created_at.clone(),
            provenance: message.provenance.clone(),
        }
    }

    pub(super) fn split(message: MessageSnapshot) -> (Self, ValueSnapshot) {
        (
            Self {
                id: message.id,
                from: message.from,
                to: message.to,
                role: message.role,
                session: message.session,
                created_at: message.created_at,
                provenance: message.provenance,
            },
            message.payload.into_value(),
        )
    }

    pub(super) fn snapshot(self, payload: SnapshotBox) -> MessageSnapshot {
        MessageSnapshot {
            id: self.id,
            from: self.from,
            to: self.to,
            role: self.role,
            session: self.session,
            created_at: self.created_at,
            provenance: self.provenance,
            payload,
        }
    }

    pub(super) fn runtime(self, payload: SharedValue) -> MessageValue {
        MessageValue {
            id: self.id,
            from: self.from,
            to: self.to,
            role: self.role,
            session: self.session,
            created_at: self.created_at,
            provenance: self.provenance,
            payload,
        }
    }
}

pub(super) struct ConversationHeader {
    selected_context: Option<crate::value::PublishedContextValue>,
    session: String,
    history_fence: Option<etas_host::session::SessionHistoryFence>,
    cursor: Option<String>,
}

impl ConversationHeader {
    pub(super) fn capture(value: &ConversationValue) -> Self {
        Self {
            selected_context: value.selected_context.clone(),
            session: value.session.clone(),
            history_fence: value.history_fence.clone(),
            cursor: value.cursor.clone(),
        }
    }

    pub(super) fn split(value: ConversationSnapshot) -> (Self, Vec<MessageSnapshot>) {
        (
            Self {
                selected_context: value.selected_context,
                session: value.session,
                history_fence: value.history_fence,
                cursor: value.cursor,
            },
            value.messages,
        )
    }

    pub(super) fn snapshot(self, messages: Vec<MessageSnapshot>) -> ValueSnapshot {
        ValueSnapshot::Conversation(ConversationSnapshot {
            selected_context: self.selected_context,
            session: self.session,
            history_fence: self.history_fence,
            cursor: self.cursor,
            messages,
        })
    }

    pub(super) fn runtime(self, messages: MessageList) -> InterpValue {
        InterpValue::Conversation(ConversationValue {
            selected_context: self.selected_context,
            session: self.session,
            history_fence: self.history_fence,
            cursor: self.cursor,
            messages,
        })
    }
}

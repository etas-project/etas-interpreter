use std::collections::HashSet;

use crate::value::{InterpValue as Value, ListIter};

#[derive(PartialEq, Eq, Hash)]
enum Identity {
    Backing(*const ()),
    Slice(*const (), usize, usize),
}

/// Query-local identities of immutable borrowed children, not value equality or
/// persisted identities. The root keeps every backing alive throughout the scan.
#[derive(Default)]
pub(super) struct Visited {
    first: Option<Identity>,
    others: HashSet<Identity>,
}

impl Visited {
    fn insert(&mut self, identity: Identity) -> bool {
        match &self.first {
            None => {
                self.first = Some(identity);
                true
            }
            Some(first) if *first == identity => false,
            Some(_) => self.others.insert(identity),
        }
    }

    pub(super) fn enter_value(&mut self, value: &Value) -> bool {
        let identity = match value {
            Value::Array(values) | Value::Stack(values) => values.shared_capture_identity(),
            Value::Tuple(values) | Value::Variant { fields: values, .. } => {
                values.shared_capture_identity()
            }
            Value::Set(values) | Value::OrderedSet(values) => values.shared_capture_identity(),
            Value::Deque(values) | Value::Queue(values) => values.shared_capture_identity(),
            Value::Map(values) | Value::OrderedMap(values) | Value::PriorityQueue(values) => {
                values.shared_capture_identity()
            }
            Value::Record(values) => values.shared_capture_identity(),
            // The caller checks Secret before reaching this child-only cache.
            Value::Trust { value, .. }
            | Value::Nominal { value, .. }
            | Value::OptionSome(value) => value.shared_capture_identity(),
            Value::Message(message) => message.payload.shared_capture_identity(),
            Value::Conversation(conversation) => conversation.messages.shared_capture_identity(),
            Value::Slice(values) => {
                return values
                    .shared_capture_identity()
                    .is_none_or(|(backing, start, end)| {
                        self.insert(Identity::Slice(backing, start, end))
                    });
            }
            // List cursors register shared suffixes individually as they advance.
            _ => None,
        };
        identity.is_none_or(|backing| self.insert(Identity::Backing(backing)))
    }

    pub(super) fn enter_list_tail(&mut self, values: &ListIter<'_>) -> bool {
        values
            .shared_tail_identity()
            .is_none_or(|backing| self.insert(Identity::Backing(backing)))
    }
}

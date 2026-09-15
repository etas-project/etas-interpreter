use std::{collections::vec_deque, rc::Rc};

use crate::value::{InterpValue as Value, ListValue};

enum Children {
    Values(std::vec::IntoIter<Value>),
    Record(std::vec::IntoIter<(String, Value)>),
    Entries {
        entries: std::vec::IntoIter<(Value, Value)>,
        value: Option<Value>,
    },
    Deque(vec_deque::IntoIter<Value>),
    List(ListValue),
    Messages(std::vec::IntoIter<crate::value::MessageValue>),
    Range {
        start: Option<Box<Value>>,
        end: Option<Box<Value>>,
    },
}

impl Children {
    fn values(values: Vec<Value>) -> Self {
        Self::Values(values.into_iter())
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Values(values) => values.len() == 0,
            Self::Record(values) => values.len() == 0,
            Self::Entries { entries, value } => entries.len() == 0 && value.is_none(),
            Self::Deque(values) => values.len() == 0,
            Self::List(values) => values.is_empty(),
            Self::Messages(values) => values.len() == 0,
            Self::Range { start, end } => start.is_none() && end.is_none(),
        }
    }
}

impl Iterator for Children {
    type Item = Value;

    fn next(&mut self) -> Option<Value> {
        match self {
            Self::Values(values) => values.next(),
            Self::Record(values) => values.next().map(|(_, value)| value),
            Self::Entries { entries, value } => {
                if value.is_some() {
                    return value.take();
                }
                let (key, next) = entries.next()?;
                *value = Some(next);
                Some(key)
            }
            Self::Deque(values) => values.next(),
            Self::List(values) => values.pop_unique_front_for_drop(),
            Self::Messages(values) => values.next().map(Value::Message),
            Self::Range { start, end } => start.take().or_else(|| end.take()).map(|value| *value),
        }
    }
}

pub(crate) fn release_value(mut current: Value) {
    // Own iterators over detached storage, not copies of all pending children.
    // Unary paths are tail-processed; additional state scales with branching
    // depth rather than collection width. Shared subtrees terminate the walk.
    let mut pending: Vec<Children> = Vec::new();
    loop {
        let children = match current {
            Value::Nominal { value, .. }
            | Value::Trust { value, .. }
            | Value::OptionSome(value) => {
                if let Ok(mut node) = Rc::try_unwrap(value.0) {
                    current = std::mem::replace(&mut node.0, Value::Unit);
                    continue;
                }
                None
            }
            Value::Tuple(fields) | Value::Variant { fields, .. } => Rc::try_unwrap(fields.0)
                .ok()
                .map(|mut node| Children::values(std::mem::take(&mut node.0))),
            Value::Array(values) | Value::Stack(values) => {
                values.into_unique_values().map(Children::values)
            }
            Value::Slice(values) => values.into_unique_backing().map(Children::values),
            Value::Set(values) | Value::OrderedSet(values) => {
                values.into_unique_values().map(Children::values)
            }
            Value::Deque(values) | Value::Queue(values) => values
                .into_unique_values()
                .map(|values| Children::Deque(values.into_iter())),
            Value::List(values) => Some(Children::List(values)),
            Value::Record(values) => values
                .into_unique_values()
                .map(|values| Children::Record(values.into_iter())),
            Value::Map(values) | Value::OrderedMap(values) | Value::PriorityQueue(values) => {
                values.into_unique_values().map(|values| Children::Entries {
                    entries: values.into_iter(),
                    value: None,
                })
            }
            Value::Message(message) => {
                if let Ok(mut node) = Rc::try_unwrap(message.payload.0) {
                    current = std::mem::replace(&mut node.0, Value::Unit);
                    continue;
                }
                None
            }
            Value::Conversation(conversation) => conversation
                .messages
                .into_unique_messages()
                .map(|messages| Children::Messages(messages.into_iter())),
            Value::Range(range) => Some(Children::Range {
                start: Some(range.start),
                end: Some(range.end),
            }),
            _ => None,
        };
        if let Some(mut children) = children
            && let Some(next) = children.next()
        {
            if !children.is_empty() {
                pending.push(children);
            }
            current = next;
            continue;
        }
        loop {
            let Some(children) = pending.last_mut() else {
                return;
            };
            if let Some(next) = children.next() {
                if children.is_empty() {
                    pending.pop();
                }
                current = next;
                break;
            }
            pending.pop();
        }
    }
}

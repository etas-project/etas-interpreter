use crate::value::{InterpValue as Value, ListIter, MessageValue};
use std::{collections::vec_deque, slice};

mod visited;
use visited::Visited;

#[cfg(test)]
mod tests;

#[cfg(test)]
thread_local! { static VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

enum Children<'a> {
    One(Option<&'a Value>),
    Two(std::array::IntoIter<&'a Value, 2>),
    Values(slice::Iter<'a, Value>),
    Deque(vec_deque::Iter<'a, Value>),
    List(ListIter<'a>),
    Fields(slice::Iter<'a, (String, Value)>),
    Pairs {
        entries: slice::Iter<'a, (Value, Value)>,
        value: Option<&'a Value>,
    },
    Messages(slice::Iter<'a, MessageValue>),
}

impl<'a> Children<'a> {
    fn of(value: &'a Value) -> Option<Self> {
        Some(match value {
            Value::Trust { value, .. }
            | Value::Nominal { value, .. }
            | Value::OptionSome(value) => Self::One(Some(value)),
            Value::Tuple(values) | Value::Variant { fields: values, .. } => {
                Self::Values(values.iter())
            }
            Value::Array(values) | Value::Stack(values) => Self::Values(values.borrow().iter()),
            Value::Slice(values) => Self::Values(values.borrow().iter()),
            Value::Set(values) | Value::OrderedSet(values) => Self::Values(values.borrow().iter()),
            Value::Deque(values) | Value::Queue(values) => Self::Deque(values.borrow().iter()),
            Value::List(values) => Self::List(values.iter()),
            Value::Map(entries) | Value::OrderedMap(entries) | Value::PriorityQueue(entries) => {
                Self::Pairs {
                    entries: entries.borrow().iter(),
                    value: None,
                }
            }
            Value::Record(fields) => Self::Fields(fields.borrow().iter()),
            Value::Range(range) => {
                Self::Two([range.start.as_ref(), range.end.as_ref()].into_iter())
            }
            Value::Message(message) => Self::One(Some(&message.payload)),
            Value::Conversation(conversation) => Self::Messages(conversation.messages.iter()),
            _ => return None,
        })
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::One(value) => value.is_none(),
            Self::Two(values) => values.len() == 0,
            Self::Values(values) => values.len() == 0,
            Self::Deque(values) => values.len() == 0,
            Self::List(values) => values.len() == 0,
            Self::Fields(values) => values.len() == 0,
            Self::Pairs { entries, value } => entries.len() == 0 && value.is_none(),
            Self::Messages(values) => values.len() == 0,
        }
    }
}

impl<'a> Children<'a> {
    fn next(&mut self, visited: &mut Visited) -> Option<&'a Value> {
        match self {
            Self::One(value) => value.take(),
            Self::Two(values) => values.next(),
            Self::Values(values) => values.next(),
            Self::Deque(values) => values.next(),
            Self::List(values) => visited
                .enter_list_tail(values)
                .then(|| values.next())
                .flatten(),
            Self::Fields(values) => values.next().map(|(_, value)| value),
            Self::Pairs { entries, value } => {
                if value.is_some() {
                    return value.take();
                }
                let (key, next) = entries.next()?;
                *value = Some(next);
                Some(key)
            }
            Self::Messages(values) => values.next().map(|message| message.payload.as_ref()),
        }
    }
}

pub(in crate::eval::method) fn contains_secret(mut value: &Value) -> bool {
    // Keep one sibling cursor inline; only nested branching needs heap storage.
    // Unary wrappers and collection width never materialize pending value lists.
    let mut pending: Option<Children<'_>> = None;
    let mut parents = Vec::new();
    let mut visited = Visited::default();
    loop {
        #[cfg(test)]
        VISITS.set(VISITS.get() + 1);
        if matches!(
            value,
            Value::Trust {
                wrapper: etas_types::TrustWrapper::Secret,
                ..
            }
        ) {
            return true;
        }
        if visited.enter_value(value)
            && let Some(mut children) = Children::of(value)
            && let Some(first) = children.next(&mut visited)
        {
            if !children.is_empty()
                && let Some(previous) = pending.replace(children)
            {
                parents.push(previous);
            }
            value = first;
            continue;
        }
        loop {
            let Some(children) = pending.as_mut() else {
                return false;
            };
            if let Some(next) = children.next(&mut visited) {
                if children.is_empty() {
                    pending = parents.pop();
                }
                value = next;
                break;
            }
            pending = parents.pop();
        }
    }
}

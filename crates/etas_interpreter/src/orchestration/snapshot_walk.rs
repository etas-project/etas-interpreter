use super::{MessageSnapshot, ValueSnapshot};
use std::slice::Iter;

impl ValueSnapshot {
    pub(crate) fn walk(&self) -> impl Iterator<Item = &Self> {
        Walk {
            next: Some(self),
            parents: Vec::new(),
        }
    }
}

struct Walk<'a> {
    next: Option<&'a ValueSnapshot>,
    parents: Vec<Children<'a>>,
}
impl<'a> Iterator for Walk<'a> {
    type Item = &'a ValueSnapshot;
    fn next(&mut self) -> Option<Self::Item> {
        let value = if let Some(value) = self.next.take() {
            value
        } else {
            loop {
                if let Some(value) = self.parents.last_mut()?.next() {
                    break value;
                }
                self.parents.pop();
            }
        };
        let mut children = children(value);
        if let Some(first) = children.next() {
            self.parents.push(children);
            self.next = Some(first);
        }
        Some(value)
    }
}

enum Children<'a> {
    Fixed(std::array::IntoIter<Option<&'a ValueSnapshot>, 2>),
    Values(Iter<'a, ValueSnapshot>),
    Pairs {
        entries: Iter<'a, (ValueSnapshot, ValueSnapshot)>,
        value: Option<&'a ValueSnapshot>,
    },
    Fields(Iter<'a, (String, ValueSnapshot)>),
    Messages(Iter<'a, MessageSnapshot>),
}
impl<'a> Iterator for Children<'a> {
    type Item = &'a ValueSnapshot;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Fixed(values) => values.find_map(|value| value),
            Self::Values(values) => values.next(),
            Self::Fields(values) => values.next().map(|(_, value)| value),
            Self::Messages(values) => values.next().map(|message| message.payload.as_ref()),
            Self::Pairs { entries, value } => {
                if let Some(value) = value.take() {
                    return Some(value);
                }
                let (key, next) = entries.next()?;
                *value = Some(next);
                Some(key)
            }
        }
    }
}
fn children(value: &ValueSnapshot) -> Children<'_> {
    use ValueSnapshot as V;
    let fixed = match value {
        V::Nominal { value, .. } | V::Trust { value, .. } | V::OptionSome(value) => {
            [Some(value.as_ref()), None]
        }
        V::Tuple(values)
        | V::Array(values)
        | V::List(values)
        | V::Slice(values)
        | V::Set(values)
        | V::Deque(values)
        | V::Queue(values)
        | V::Stack(values)
        | V::OrderedSet(values)
        | V::Variant { fields: values, .. } => return Children::Values(values.iter()),
        V::Map(values) | V::OrderedMap(values) | V::PriorityQueue(values) => {
            return Children::Pairs {
                entries: values.iter(),
                value: None,
            };
        }
        V::Record(values) => return Children::Fields(values.iter()),
        V::Range { start, end, .. } => [Some(start.as_ref()), Some(end.as_ref())],
        V::Message(message) => [Some(message.payload.as_ref()), None],
        V::Conversation(conversation) => return Children::Messages(conversation.messages.iter()),
        V::MemorySelection { predicate, .. } => [predicate.as_deref(), None],
        _ => [None, None],
    };
    Children::Fixed(fixed.into_iter())
}

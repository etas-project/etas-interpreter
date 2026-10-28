use crate::orchestration::{MessageSnapshot, ValueSnapshot as Value};

enum Children {
    Locals(std::vec::IntoIter<(etas_hir::SymbolId, Value)>),
    Targets(std::vec::IntoIter<crate::orchestration::CallTargetSnapshot>),
    Values(std::vec::IntoIter<Value>),
    OrderedValues {
        values: std::vec::IntoIter<Value>,
        comparator: Option<Value>,
    },
    Fields(std::vec::IntoIter<(String, Value)>),
    Pairs {
        pairs: std::vec::IntoIter<(Value, Value)>,
        pending: Option<Value>,
    },
    Range {
        start: Option<Value>,
        end: Option<Value>,
    },
    Messages(std::vec::IntoIter<MessageSnapshot>),
}

impl Children {
    fn is_empty(&self) -> bool {
        match self {
            Self::Locals(values) => values.len() == 0,
            Self::Targets(values) => values.len() == 0,
            Self::Values(values) => values.len() == 0,
            Self::OrderedValues { values, comparator } => values.len() == 0 && comparator.is_none(),
            Self::Fields(values) => values.len() == 0,
            Self::Pairs { pairs, pending } => pairs.len() == 0 && pending.is_none(),
            Self::Range { start, end } => start.is_none() && end.is_none(),
            Self::Messages(values) => values.len() == 0,
        }
    }
}

impl Iterator for Children {
    type Item = Value;
    fn next(&mut self) -> Option<Value> {
        match self {
            Self::Locals(values) => values.next().map(|(_, value)| value),
            Self::Targets(values) => values.next().map(Value::Callable),
            Self::Values(values) => values.next(),
            Self::OrderedValues { values, comparator } => {
                comparator.take().or_else(|| values.next())
            }
            Self::Fields(values) => values.next().map(|(_, value)| value),
            Self::Pairs { pairs, pending } => {
                if let Some(value) = pending.take() {
                    return Some(value);
                }
                let (key, value) = pairs.next()?;
                *pending = Some(value);
                Some(key)
            }
            Self::Range { start, end } => start.take().or_else(|| end.take()),
            Self::Messages(values) => values.find_map(|message| message.payload.into_unique()),
        }
    }
}

pub(super) fn release_value(mut current: Value) {
    // Keep one cursor inline: flat containers allocate no traversal buffers.
    let mut active: Option<Children> = None;
    let mut ancestors = Vec::new();
    loop {
        let children = match current {
            Value::Callable(crate::orchestration::CallTargetSnapshot::Lambda {
                captured, ..
            }) => captured
                .into_unique_values()
                .map(|values| Children::Locals(values.into_iter())),
            Value::Callable(
                crate::orchestration::CallTargetSnapshot::Scoped { target, .. }
                | crate::orchestration::CallTargetSnapshot::Specialized { target, .. }
                | crate::orchestration::CallTargetSnapshot::Limited { target, .. },
            ) => {
                if let Some(target) = target.into_unique() {
                    current = Value::Callable(target);
                    continue;
                }
                None
            }
            Value::Callable(crate::orchestration::CallTargetSnapshot::Composed(targets)) => targets
                .into_unique()
                .map(|values| Children::Targets(values.into_iter())),
            Value::Nominal { value, .. }
            | Value::Trust { value, .. }
            | Value::OptionSome(value)
            | Value::MemorySelection {
                predicate: Some(value),
                ..
            } => {
                if let Some(value) = value.into_unique() {
                    current = value;
                    continue;
                }
                None
            }
            Value::Message(message) => {
                if let Some(value) = message.payload.into_unique() {
                    current = value;
                    continue;
                }
                None
            }
            Value::Tuple(values)
            | Value::Array(values)
            | Value::List(values)
            | Value::Slice(values)
            | Value::Set(values)
            | Value::Deque(values)
            | Value::Queue(values)
            | Value::Stack(values)
            | Value::Variant { fields: values, .. } => values
                .into_unique()
                .map(|v| Children::Values(v.into_iter())),
            Value::OrderedSet { values, comparator } => {
                values.into_unique().map(|v| Children::OrderedValues {
                    values: v.into_iter(),
                    comparator: comparator.and_then(|v| v.into_unique()),
                })
            }
            Value::PriorityQueue {
                entries,
                comparator,
            }
            | Value::OrderedMap {
                entries,
                comparator,
            } => entries.into_unique().map(|v| Children::Pairs {
                pairs: v.into_iter(),
                pending: comparator.and_then(|v| v.into_unique()),
            }),
            Value::Map(values) => values.into_unique().map(|v| Children::Pairs {
                pairs: v.into_iter(),
                pending: None,
            }),
            Value::Record(values) => values
                .into_unique()
                .map(|v| Children::Fields(v.into_iter())),
            Value::Range { start, end, .. } => Some(Children::Range {
                start: start.into_unique(),
                end: end.into_unique(),
            }),
            Value::Conversation(value) => Some(Children::Messages(value.messages.into_iter())),
            _ => None,
        };
        if let Some(mut children) = children
            && let Some(next) = children.next()
        {
            if !children.is_empty()
                && let Some(parent) = active.replace(children)
            {
                ancestors.push(parent);
            }
            current = next;
            continue;
        }
        loop {
            let Some(children) = active.as_mut() else {
                return;
            };
            if let Some(next) = children.next() {
                if children.is_empty() {
                    active = ancestors.pop();
                }
                current = next;
                break;
            }
            active = ancestors.pop();
        }
    }
}

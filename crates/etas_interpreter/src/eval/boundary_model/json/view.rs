use etas_host::HostJsonValue;
use serde_json::Value;

#[derive(Clone, Copy)]
pub(super) enum JsonRef<'a> {
    Serde(&'a Value),
    Host(&'a HostJsonValue),
    String(&'a str),
}

impl<'a> JsonRef<'a> {
    pub fn as_str(self) -> Option<&'a str> {
        match self {
            Self::Serde(value) => value.as_str(),
            Self::Host(HostJsonValue::String(value)) => Some(value),
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(self) -> Option<bool> {
        match self {
            Self::Serde(value) => value.as_bool(),
            Self::Host(HostJsonValue::Bool(value)) => Some(*value),
            _ => None,
        }
    }

    pub fn is_null(self) -> bool {
        matches!(
            self,
            Self::Serde(Value::Null) | Self::Host(HostJsonValue::Null)
        )
    }

    pub fn as_f64(self) -> Option<f64> {
        match self {
            Self::Serde(value) => value.as_f64(),
            Self::Host(HostJsonValue::Number(value)) if value.is_finite() => Some(*value),
            _ => None,
        }
    }

    pub fn as_i64(self) -> Option<i64> {
        match self {
            Self::Serde(value) => value.as_i64(),
            // Preserve the existing floating JSON number ABI, including its
            // distinction from integral serde numbers. Do not round or truncate.
            Self::Host(HostJsonValue::Number(value)) => {
                serde_json::Number::from_f64(*value)?.as_i64()
            }
            _ => None,
        }
    }

    pub fn as_u64(self) -> Option<u64> {
        match self {
            Self::Serde(value) => value.as_u64(),
            Self::Host(HostJsonValue::Number(value)) => {
                serde_json::Number::from_f64(*value)?.as_u64()
            }
            _ => None,
        }
    }

    pub fn as_array(self) -> Option<JsonArray<'a>> {
        match self {
            Self::Serde(Value::Array(values)) => Some(JsonArray::Serde(values)),
            Self::Host(HostJsonValue::Array(values)) => Some(JsonArray::Host(values)),
            _ => None,
        }
    }

    pub fn as_object(self) -> Option<JsonObject<'a>> {
        match self {
            Self::Serde(Value::Object(values)) => Some(JsonObject::Serde(values)),
            Self::Host(HostJsonValue::Object(values)) => {
                let mut entries: Vec<_> = values
                    .iter()
                    .enumerate()
                    .map(|(ordinal, (name, value))| HostEntry {
                        name,
                        value,
                        ordinal,
                    })
                    .collect();
                // Match serde's canonical map ordering and last-value-wins
                // duplicate handling, without cloning keys or child graphs.
                entries.sort_unstable_by(|a, b| a.name.cmp(b.name).then(b.ordinal.cmp(&a.ordinal)));
                entries.dedup_by(|a, b| a.name == b.name);
                Some(JsonObject::Host(entries))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum JsonArray<'a> {
    Serde(&'a [Value]),
    Host(&'a [HostJsonValue]),
}

impl<'a> JsonArray<'a> {
    pub fn len(self) -> usize {
        match self {
            Self::Serde(values) => values.len(),
            Self::Host(values) => values.len(),
        }
    }
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
    pub fn get(self, index: usize) -> Option<JsonRef<'a>> {
        match self {
            Self::Serde(values) => values.get(index).map(JsonRef::Serde),
            Self::Host(values) => values.get(index).map(JsonRef::Host),
        }
    }
    pub fn iter(self) -> impl ExactSizeIterator<Item = JsonRef<'a>> {
        (0..self.len()).map(move |index| match self {
            Self::Serde(values) => JsonRef::Serde(&values[index]),
            Self::Host(values) => JsonRef::Host(&values[index]),
        })
    }
}

pub(super) struct HostEntry<'a> {
    name: &'a str,
    value: &'a HostJsonValue,
    ordinal: usize,
}

pub(super) enum JsonObject<'a> {
    Serde(&'a serde_json::Map<String, Value>),
    Host(Vec<HostEntry<'a>>),
}

impl JsonObject<'_> {
    pub fn get(&self, name: &str) -> Option<JsonRef<'_>> {
        match self {
            Self::Serde(values) => values.get(name).map(JsonRef::Serde),
            Self::Host(entries) => entries
                .binary_search_by(|entry| entry.name.cmp(name))
                .ok()
                .map(|slot| JsonRef::Host(entries[slot].value)),
        }
    }

    pub fn iter(&self) -> ObjectIter<'_> {
        match self {
            Self::Serde(values) => ObjectIter::Serde(values.iter()),
            Self::Host(entries) => ObjectIter::Host(entries.iter()),
        }
    }
}

pub(super) enum ObjectIter<'a> {
    Serde(serde_json::map::Iter<'a>),
    Host(std::slice::Iter<'a, HostEntry<'a>>),
}

impl<'a> Iterator for ObjectIter<'a> {
    type Item = (&'a str, JsonRef<'a>);
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Serde(values) => values
                .next()
                .map(|(name, value)| (name.as_str(), JsonRef::Serde(value))),
            Self::Host(entries) => entries
                .next()
                .map(|entry| (entry.name, JsonRef::Host(entry.value))),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Serde(values) => values.size_hint(),
            Self::Host(entries) => entries.size_hint(),
        }
    }
}
impl ExactSizeIterator for ObjectIter<'_> {}

enum Children<'a> {
    Array(std::slice::Iter<'a, HostJsonValue>),
    Object(std::slice::Iter<'a, (String, HostJsonValue)>),
}

impl<'a> Iterator for Children<'a> {
    type Item = &'a HostJsonValue;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Array(values) => values.next(),
            Self::Object(values) => values.next().map(|(_, value)| value),
        }
    }
}

pub(super) fn valid_host_json(root: &HostJsonValue) -> bool {
    let mut pending: Vec<Children<'_>> = Vec::new();
    let mut next = Some(root);
    loop {
        if let Some(node) = next.take() {
            match node {
                HostJsonValue::Number(value) if !value.is_finite() => return false,
                HostJsonValue::Array(values) => pending.push(Children::Array(values.iter())),
                HostJsonValue::Object(values) => pending.push(Children::Object(values.iter())),
                _ => {}
            }
        }
        match pending.last_mut() {
            Some(children) => match children.next() {
                Some(child) => next = Some(child),
                None => {
                    pending.pop();
                }
            },
            None => return true,
        }
    }
}

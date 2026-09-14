use super::ValueSnapshot;
use std::ops::{Deref, DerefMut};

// Recursive snapshot edges own their data, but release descendants on a worklist.
// This also protects partially built snapshots when capture/restore fails.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SnapshotBox(Box<ValueSnapshot>);

impl SnapshotBox {
    pub(crate) fn new(value: ValueSnapshot) -> Self {
        Self(Box::new(value))
    }
    pub(crate) fn into_value(mut self) -> ValueSnapshot {
        std::mem::replace(self.0.as_mut(), ValueSnapshot::Unit)
    }
}
impl Deref for SnapshotBox {
    type Target = ValueSnapshot;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for SnapshotBox {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl AsRef<ValueSnapshot> for SnapshotBox {
    fn as_ref(&self) -> &ValueSnapshot {
        self
    }
}
impl AsMut<ValueSnapshot> for SnapshotBox {
    fn as_mut(&mut self) -> &mut ValueSnapshot {
        self
    }
}
impl Drop for SnapshotBox {
    fn drop(&mut self) {
        let value = std::mem::replace(self.0.as_mut(), ValueSnapshot::Unit);
        release(std::iter::once(value));
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SnapshotChildren<T: SnapshotChild>(Vec<T>);
impl<T: SnapshotChild> SnapshotChildren<T> {
    pub(crate) fn into_values(mut self) -> Vec<T> {
        std::mem::take(&mut self.0)
    }
}
impl<T: SnapshotChild> From<Vec<T>> for SnapshotChildren<T> {
    fn from(values: Vec<T>) -> Self {
        Self(values)
    }
}
impl<T: SnapshotChild> FromIterator<T> for SnapshotChildren<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}
impl<T: SnapshotChild> Deref for SnapshotChildren<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<T: SnapshotChild> DerefMut for SnapshotChildren<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl<T: SnapshotChild> IntoIterator for SnapshotChildren<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.into_values().into_iter()
    }
}
impl<'a, T: SnapshotChild> IntoIterator for &'a SnapshotChildren<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
impl<'a, T: SnapshotChild> IntoIterator for &'a mut SnapshotChildren<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
    }
}
impl<T: SnapshotChild> Drop for SnapshotChildren<T> {
    fn drop(&mut self) {
        release(self.0.drain(..));
    }
}

pub(crate) trait SnapshotChild {
    fn detach(self, pending: &mut Vec<ValueSnapshot>);
}
impl SnapshotChild for (ValueSnapshot, ValueSnapshot) {
    fn detach(self, pending: &mut Vec<ValueSnapshot>) {
        self.0.detach(pending);
        self.1.detach(pending);
    }
}
impl SnapshotChild for (String, ValueSnapshot) {
    fn detach(self, pending: &mut Vec<ValueSnapshot>) {
        self.1.detach(pending);
    }
}
impl SnapshotChild for ValueSnapshot {
    fn detach(self, pending: &mut Vec<ValueSnapshot>) {
        use ValueSnapshot as V;
        match self {
            V::Nominal { value, .. } | V::Trust { value, .. } | V::OptionSome(value) => {
                pending.push(value.into_value())
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
            | V::Variant { fields: values, .. } => pending.extend(values.into_values()),
            V::Map(values) | V::OrderedMap(values) | V::PriorityQueue(values) => {
                for (key, value) in values.into_values() {
                    pending.push(key);
                    pending.push(value);
                }
            }
            V::Record(values) => {
                pending.extend(values.into_values().into_iter().map(|(_, value)| value))
            }
            V::Range { start, end, .. } => {
                pending.push(start.into_value());
                pending.push(end.into_value());
            }
            V::Message(message) => pending.push(message.payload.into_value()),
            V::Conversation(conversation) => pending.extend(
                conversation
                    .messages
                    .into_iter()
                    .map(|message| message.payload.into_value()),
            ),
            V::MemorySelection {
                predicate: Some(value),
                ..
            } => pending.push(value.into_value()),
            _ => {}
        }
    }
}
fn release<T: SnapshotChild>(values: impl Iterator<Item = T>) {
    let mut pending = Vec::new();
    for value in values {
        value.detach(&mut pending);
        while let Some(value) = pending.pop() {
            value.detach(&mut pending);
        }
    }
}

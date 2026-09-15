use super::MessageValue;
use crate::value::{InterpValue, shared::release_value};
use std::{ops::Deref, rc::Rc};

/// Immutable selected history. Owned extraction copies headers only when shared.
#[derive(Clone, Debug, Default)]
pub struct MessageList(Rc<Messages>);

#[derive(Debug, Default)]
struct Messages(Vec<MessageValue>);

impl PartialEq for MessageList {
    fn eq(&self, other: &Self) -> bool {
        crate::value::comparison::messages_equal(self, other)
    }
}

impl Eq for MessageList {}

impl MessageList {
    pub fn into_messages(self) -> Vec<MessageValue> {
        match Rc::try_unwrap(self.0) {
            Ok(mut node) => std::mem::take(&mut node.0),
            Err(node) => node.0.clone(),
        }
    }

    pub(crate) fn into_unique_messages(self) -> Option<Vec<MessageValue>> {
        Rc::try_unwrap(self.0)
            .ok()
            .map(|mut node| std::mem::take(&mut node.0))
    }
}

impl From<Vec<MessageValue>> for MessageList {
    fn from(value: Vec<MessageValue>) -> Self {
        Self(Rc::new(Messages(value)))
    }
}

impl FromIterator<MessageValue> for MessageList {
    fn from_iter<T: IntoIterator<Item = MessageValue>>(iter: T) -> Self {
        Vec::from_iter(iter).into()
    }
}

impl Deref for MessageList {
    type Target = [MessageValue];
    fn deref(&self) -> &Self::Target {
        &self.0.0
    }
}

impl IntoIterator for MessageList {
    type Item = MessageValue;
    type IntoIter = std::vec::IntoIter<MessageValue>;
    fn into_iter(self) -> Self::IntoIter {
        self.into_messages().into_iter()
    }
}

impl<'a> IntoIterator for &'a MessageList {
    type Item = &'a MessageValue;
    type IntoIter = std::slice::Iter<'a, MessageValue>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl Drop for Messages {
    fn drop(&mut self) {
        for message in std::mem::take(&mut self.0) {
            release_value(InterpValue::Message(message));
        }
    }
}

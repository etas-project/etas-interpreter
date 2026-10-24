use std::{fmt, ops::Deref, rc::Rc};

use super::{PromptMessage, StringValue};

/// Immutable messages may be shared by values, continuations and saved snapshots.
/// No mutable reference to this backing escapes; updates detach the message table
/// while each message's immutable text remains shared until it too is changed.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct PromptValue(Rc<Vec<PromptMessage>>);

impl PromptValue {
    pub fn new(messages: Vec<PromptMessage>) -> Self {
        Self(Rc::new(messages))
    }

    pub fn push(&mut self, message: PromptMessage) {
        if let Some(messages) = Rc::get_mut(&mut self.0) {
            messages.push(message);
        } else {
            let mut messages = Vec::with_capacity(self.0.len() + 1);
            messages.extend(self.0.iter().cloned());
            messages.push(message);
            self.0 = Rc::new(messages);
        }
    }

    pub fn into_values(self) -> Vec<PromptMessage> {
        Rc::unwrap_or_clone(self.0)
    }

    pub(crate) fn into_text(self) -> StringValue {
        match Rc::try_unwrap(self.0) {
            Ok(messages) => {
                let mut messages = messages.into_iter();
                let Some(first) = messages.next() else {
                    return StringValue::default();
                };
                let mut text = first.text;
                for message in messages {
                    text.push_str("\n");
                    text.push_str(&message.text);
                }
                text
            }
            Err(messages) => {
                let mut messages = messages.iter();
                let Some(first) = messages.next() else {
                    return StringValue::default();
                };
                if messages.len() == 0 {
                    return first.text.clone();
                }
                let mut text = first.text.as_str().to_owned();
                for message in messages {
                    text.push('\n');
                    text.push_str(&message.text);
                }
                text.into()
            }
        }
    }
}

impl Deref for PromptValue {
    type Target = [PromptMessage];
    fn deref(&self) -> &[PromptMessage] {
        self.0.as_slice()
    }
}

impl From<Vec<PromptMessage>> for PromptValue {
    fn from(messages: Vec<PromptMessage>) -> Self {
        Self::new(messages)
    }
}

impl FromIterator<PromptMessage> for PromptValue {
    fn from_iter<T: IntoIterator<Item = PromptMessage>>(messages: T) -> Self {
        Self::new(messages.into_iter().collect())
    }
}

impl IntoIterator for PromptValue {
    type Item = PromptMessage;
    type IntoIter = std::vec::IntoIter<PromptMessage>;
    fn into_iter(self) -> Self::IntoIter {
        self.into_values().into_iter()
    }
}

impl<'a> IntoIterator for &'a PromptValue {
    type Item = &'a PromptMessage;
    type IntoIter = std::slice::Iter<'a, PromptMessage>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl fmt::Debug for PromptValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, formatter)
    }
}

#[cfg(test)]
mod tests;

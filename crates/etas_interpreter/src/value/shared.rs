use std::{fmt, ops::Deref, rc::Rc};

use super::InterpValue;
mod release;
use release::release_value;

#[cfg(test)]
mod tests;

#[derive(Clone, PartialEq, Eq)]
pub struct SharedValue(Rc<ValueNode>);

#[derive(Clone, PartialEq, Eq)]
struct ValueNode(InterpValue);

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SharedFields(Rc<FieldsNode>);

#[derive(Clone, Default, PartialEq, Eq)]
struct FieldsNode(Vec<InterpValue>);

impl SharedValue {
    pub fn new(value: InterpValue) -> Self {
        Self(Rc::new(ValueNode(value)))
    }

    pub fn into_value(self) -> InterpValue {
        match Rc::try_unwrap(self.0) {
            Ok(mut node) => std::mem::replace(&mut node.0, InterpValue::Unit),
            Err(node) => node.0.clone(),
        }
    }

    pub(crate) fn make_mut(&mut self) -> &mut InterpValue {
        &mut Rc::make_mut(&mut self.0).0
    }
}

impl From<InterpValue> for SharedValue {
    fn from(value: InterpValue) -> Self {
        Self::new(value)
    }
}

impl Deref for SharedValue {
    type Target = InterpValue;
    fn deref(&self) -> &Self::Target {
        &self.0.0
    }
}

impl AsRef<InterpValue> for SharedValue {
    fn as_ref(&self) -> &InterpValue {
        self
    }
}

impl SharedFields {
    pub fn as_slice(&self) -> &[InterpValue] {
        self
    }

    pub fn new(values: Vec<InterpValue>) -> Self {
        Self(Rc::new(FieldsNode(values)))
    }

    pub fn into_values(self) -> Vec<InterpValue> {
        match Rc::try_unwrap(self.0) {
            Ok(mut node) => std::mem::take(&mut node.0),
            Err(node) => node.0.clone(),
        }
    }

    pub(crate) fn into_single(self) -> Option<InterpValue> {
        if self.len() != 1 {
            return None;
        }
        match Rc::try_unwrap(self.0) {
            Ok(mut node) => node.0.pop(),
            Err(node) => node.0.first().cloned(),
        }
    }
}

impl From<Vec<InterpValue>> for SharedFields {
    fn from(values: Vec<InterpValue>) -> Self {
        Self::new(values)
    }
}

impl Deref for SharedFields {
    type Target = [InterpValue];
    fn deref(&self) -> &Self::Target {
        &self.0.0
    }
}

impl IntoIterator for SharedFields {
    type Item = InterpValue;
    type IntoIter = std::vec::IntoIter<InterpValue>;
    fn into_iter(self) -> Self::IntoIter {
        self.into_values().into_iter()
    }
}

impl<'a> IntoIterator for &'a SharedFields {
    type Item = &'a InterpValue;
    type IntoIter = std::slice::Iter<'a, InterpValue>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl fmt::Debug for SharedValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl fmt::Debug for SharedFields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl Drop for ValueNode {
    fn drop(&mut self) {
        release_value(std::mem::replace(&mut self.0, InterpValue::Unit));
    }
}

impl Drop for FieldsNode {
    fn drop(&mut self) {
        for value in std::mem::take(&mut self.0) {
            release_value(value);
        }
    }
}

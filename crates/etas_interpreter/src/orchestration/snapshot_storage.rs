use super::ValueSnapshot;
use std::{
    ops::{Deref, DerefMut},
    rc::Rc,
};

mod release;
#[cfg(test)]
mod tests;

// Snapshot edges share only captured data. Mutable access detaches the immediate
// node; the last owner releases descendants on a worklist, including partial trees.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SnapshotBox(Rc<ValueNode>);

#[derive(Debug, Clone, PartialEq)]
struct ValueNode(ValueSnapshot);

impl SnapshotBox {
    pub(crate) fn new(value: ValueSnapshot) -> Self {
        Self(Rc::new(ValueNode(value)))
    }

    pub(crate) fn into_value(self) -> ValueSnapshot {
        match Rc::try_unwrap(self.0) {
            Ok(mut node) => std::mem::replace(&mut node.0, ValueSnapshot::Unit),
            Err(node) => node.0.clone(),
        }
    }

    fn into_unique(self) -> Option<ValueSnapshot> {
        Rc::try_unwrap(self.0)
            .ok()
            .map(|mut node| std::mem::replace(&mut node.0, ValueSnapshot::Unit))
    }
}

impl Deref for SnapshotBox {
    type Target = ValueSnapshot;
    fn deref(&self) -> &Self::Target {
        &self.0.0
    }
}
impl DerefMut for SnapshotBox {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut Rc::make_mut(&mut self.0).0
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
impl Drop for ValueNode {
    fn drop(&mut self) {
        release::release_value(std::mem::replace(&mut self.0, ValueSnapshot::Unit));
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SnapshotChildren<T: SnapshotChild>(Rc<ChildrenNode<T>>);

#[derive(Debug, Clone, PartialEq)]
struct ChildrenNode<T: SnapshotChild>(Vec<T>);

impl<T: SnapshotChild> SnapshotChildren<T> {
    pub(crate) fn into_values(self) -> Vec<T> {
        match Rc::try_unwrap(self.0) {
            Ok(mut node) => std::mem::take(&mut node.0),
            Err(node) => node.0.clone(),
        }
    }

    fn into_unique(self) -> Option<Vec<T>> {
        Rc::try_unwrap(self.0)
            .ok()
            .map(|mut node| std::mem::take(&mut node.0))
    }
}
impl<T: SnapshotChild> From<Vec<T>> for SnapshotChildren<T> {
    fn from(values: Vec<T>) -> Self {
        Self(Rc::new(ChildrenNode(values)))
    }
}
impl<T: SnapshotChild> FromIterator<T> for SnapshotChildren<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Vec::from_iter(iter).into()
    }
}
impl<T: SnapshotChild> Deref for SnapshotChildren<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Self::Target {
        &self.0.0
    }
}
impl<T: SnapshotChild> DerefMut for SnapshotChildren<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut Rc::make_mut(&mut self.0).0
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
        self.iter()
    }
}
impl<'a, T: SnapshotChild> IntoIterator for &'a mut SnapshotChildren<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
impl<T: SnapshotChild> Drop for ChildrenNode<T> {
    fn drop(&mut self) {
        for value in std::mem::take(&mut self.0) {
            value.release();
        }
    }
}

pub(crate) trait SnapshotChild: Clone {
    fn release(self);
}
impl SnapshotChild for (ValueSnapshot, ValueSnapshot) {
    fn release(self) {
        release::release_value(self.0);
        release::release_value(self.1);
    }
}
impl SnapshotChild for (String, ValueSnapshot) {
    fn release(self) {
        release::release_value(self.1);
    }
}
impl SnapshotChild for ValueSnapshot {
    fn release(self) {
        release::release_value(self);
    }
}

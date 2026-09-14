use std::{fmt, rc::Rc};

use super::InterpValue;

#[cfg(test)]
thread_local! {
    static NODE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Persistent cons cells. Copies share their tail; indexed writes copy only
/// the path to the changed cell. No reference into a cell survives a mutation.
#[derive(Clone, Default)]
pub struct ListValue {
    head: Link,
    len: usize,
}

#[derive(Clone, Default)]
struct Link(Option<Rc<Node>>);

#[derive(Clone)]
struct Node {
    value: InterpValue,
    next: Link,
}

impl Drop for Link {
    fn drop(&mut self) {
        // Releasing a long unique tail must not recurse through Rc destructors.
        let mut next = self.0.take();
        while let Some(node) = next {
            match Rc::try_unwrap(node) {
                Ok(mut node) => next = node.next.0.take(),
                Err(_) => break,
            }
        }
    }
}

impl ListValue {
    pub(super) fn pop_unique_front_for_drop(&mut self) -> Option<InterpValue> {
        let head = self.head.0.take()?;
        match Rc::try_unwrap(head) {
            Ok(Node { value, next }) => {
                self.head = next;
                self.len -= 1;
                Some(value)
            }
            Err(_) => {
                // Another list owns the suffix. Do not clone or drain it.
                self.len = 0;
                None
            }
        }
    }

    pub fn new(values: Vec<InterpValue>) -> Self {
        let mut list = Self::default();
        for value in values.into_iter().rev() {
            list.push_front(value);
        }
        list
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> ListIter<'_> {
        ListIter {
            next: self.head.0.as_deref(),
            remaining: self.len,
        }
    }

    pub fn get(&self, index: usize) -> Option<&InterpValue> {
        self.iter().nth(index)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut InterpValue> {
        if index >= self.len {
            return None;
        }
        let mut link = &mut self.head;
        for _ in 0..index {
            link = &mut Rc::make_mut(link.0.as_mut()?).next;
        }
        Some(&mut Rc::make_mut(link.0.as_mut()?).value)
    }

    pub fn push_front(&mut self, value: InterpValue) {
        self.head = Link(Some(Rc::new(Node {
            value,
            next: std::mem::take(&mut self.head),
        })));
        self.len += 1;
    }

    pub fn pop_front(&mut self) -> Option<InterpValue> {
        let head = self.head.0.take()?;
        let Node { value, next } = Rc::unwrap_or_clone(head);
        self.head = next;
        self.len -= 1;
        Some(value)
    }

    pub fn append(&mut self, other: Self) {
        if other.is_empty() {
            return;
        }
        let mut link = &mut self.head;
        while let Some(ref mut node) = link.0 {
            link = &mut Rc::make_mut(node).next;
        }
        *link = other.head;
        self.len += other.len;
    }

    /// Advance a borrowed traversal without copying the discarded payload.
    pub(crate) fn advance(&mut self) -> bool {
        let Some(head) = &self.head.0 else {
            return false;
        };
        #[cfg(test)]
        NODE_VISITS.set(NODE_VISITS.get() + 1);
        self.head = head.next.clone();
        self.len -= 1;
        true
    }

    pub fn snapshot(&self) -> Vec<InterpValue> {
        self.iter().cloned().collect()
    }

    pub fn into_values(mut self) -> Vec<InterpValue> {
        let mut values = Vec::with_capacity(self.len);
        while let Some(value) = self.pop_front() {
            values.push(value);
        }
        values
    }
}

impl From<Vec<InterpValue>> for ListValue {
    fn from(values: Vec<InterpValue>) -> Self {
        Self::new(values)
    }
}

impl fmt::Debug for ListValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl PartialEq for ListValue {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().eq(other.iter())
    }
}

impl Eq for ListValue {}

pub struct ListIter<'a> {
    next: Option<&'a Node>,
    remaining: usize,
}

impl<'a> Iterator for ListIter<'a> {
    type Item = &'a InterpValue;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.next?;
        #[cfg(test)]
        NODE_VISITS.set(NODE_VISITS.get() + 1);
        self.next = node.next.0.as_deref();
        self.remaining -= 1;
        Some(&node.value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for ListIter<'_> {}
impl std::iter::FusedIterator for ListIter<'_> {}

#[cfg(test)]
mod tests;

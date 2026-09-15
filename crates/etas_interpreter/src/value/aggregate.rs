use std::{
    cell::{Ref, RefCell, RefMut},
    fmt,
    rc::Rc,
};

use super::primitive::InterpValue;

#[derive(Clone)]
pub struct ArrayValue(Rc<RefCell<Vec<InterpValue>>>);

impl ArrayValue {
    pub(super) fn into_unique_values(self) -> Option<Vec<InterpValue>> {
        Rc::try_unwrap(self.0).ok().map(RefCell::into_inner)
    }

    pub fn new(values: Vec<InterpValue>) -> Self {
        Self(Rc::new(RefCell::new(values)))
    }

    pub fn borrow(&self) -> Ref<'_, Vec<InterpValue>> {
        self.0.borrow()
    }

    pub fn borrow_mut(&mut self) -> RefMut<'_, Vec<InterpValue>> {
        self.make_unique();
        self.0.borrow_mut()
    }

    pub fn snapshot(&self) -> Vec<InterpValue> {
        self.borrow().clone()
    }

    /// Reuse unique backing; shared values retain their original snapshot.
    pub fn into_values(self) -> Vec<InterpValue> {
        match Rc::try_unwrap(self.0) {
            Ok(values) => values.into_inner(),
            Err(shared) => shared.borrow().clone(),
        }
    }

    pub(crate) fn push(&mut self, value: InterpValue) {
        self.prepare_growth(1);
        self.0.borrow_mut().push(value);
    }

    fn prepare_growth(&mut self, additional: usize) {
        if Rc::strong_count(&self.0) > 1 {
            let current = self.0.borrow();
            // Clone shared headers directly into the final capacity, rather
            // than cloning a full buffer and immediately growing it again.
            let mut next = Vec::with_capacity(current.len() + additional);
            next.extend(current.iter().cloned());
            drop(current);
            self.0 = Rc::new(RefCell::new(next));
        }
    }

    pub(crate) fn pop(&mut self) -> Option<InterpValue> {
        if self.borrow().is_empty() {
            return None;
        }
        self.borrow_mut().pop()
    }

    /// Consume unique buffers and clone shared elements directly into the
    /// final buffer. In particular, a shared right operand needs no temporary Vec.
    pub(crate) fn concat(mut self, other: Self) -> Self {
        let right_len = other.borrow().len();
        if right_len == 0 {
            return self;
        }
        if self.borrow().is_empty() {
            return other;
        }
        self.prepare_growth(right_len);
        let mut values = self.0.borrow_mut();
        values.reserve(right_len);
        match Rc::try_unwrap(other.0) {
            Ok(right) => values.extend(right.into_inner()),
            Err(shared) => values.extend(shared.borrow().iter().cloned()),
        }
        drop(values);
        self
    }

    pub fn make_unique(&mut self) {
        if Rc::strong_count(&self.0) > 1 {
            self.0 = Rc::new(RefCell::new(self.snapshot()));
        }
    }
}

impl fmt::Debug for ArrayValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ArrayValue").field(&self.borrow()).finish()
    }
}

impl PartialEq for ArrayValue {
    fn eq(&self, other: &Self) -> bool {
        *self.borrow() == *other.borrow()
    }
}

impl Eq for ArrayValue {}

impl From<Vec<InterpValue>> for ArrayValue {
    fn from(values: Vec<InterpValue>) -> Self {
        Self::new(values)
    }
}

#[derive(Clone)]
pub struct SliceValue {
    backing: Rc<RefCell<Vec<InterpValue>>>,
    range: std::ops::Range<usize>,
}

impl SliceValue {
    pub(super) fn into_unique_backing(self) -> Option<Vec<InterpValue>> {
        Rc::try_unwrap(self.backing).ok().map(RefCell::into_inner)
    }

    pub fn new(values: Vec<InterpValue>) -> Self {
        Self {
            range: 0..values.len(),
            backing: Rc::new(RefCell::new(values)),
        }
    }

    pub fn from_array(values: ArrayValue, range: std::ops::Range<usize>) -> Option<Self> {
        if range.start > range.end || range.end > values.borrow().len() {
            return None;
        }
        Some(Self {
            backing: values.0,
            range,
        })
    }

    pub fn slice(self, range: std::ops::Range<usize>) -> Option<Self> {
        if range.start > range.end || range.end > self.range.len() {
            return None;
        }
        Some(Self {
            range: self.range.start + range.start..self.range.start + range.end,
            backing: self.backing,
        })
    }

    pub fn borrow(&self) -> Ref<'_, [InterpValue]> {
        Ref::map(self.backing.borrow(), |values| &values[self.range.clone()])
    }

    pub fn snapshot(&self) -> Vec<InterpValue> {
        self.borrow().to_vec()
    }

    /// Reuse unique backing; shared values retain their original snapshot.
    pub fn into_values(self) -> Vec<InterpValue> {
        match Rc::try_unwrap(self.backing) {
            Ok(values) => {
                let mut values = values.into_inner();
                values.truncate(self.range.end);
                values.drain(..self.range.start);
                values
            }
            Err(shared) => shared.borrow()[self.range].to_vec(),
        }
    }
}

impl fmt::Debug for SliceValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SliceValue").field(&self.borrow()).finish()
    }
}

impl PartialEq for SliceValue {
    fn eq(&self, other: &Self) -> bool {
        *self.borrow() == *other.borrow()
    }
}

impl Eq for SliceValue {}

impl From<Vec<InterpValue>> for SliceValue {
    fn from(values: Vec<InterpValue>) -> Self {
        Self::new(values)
    }
}

#[cfg(test)]
#[path = "aggregate/tests.rs"]
mod tests;

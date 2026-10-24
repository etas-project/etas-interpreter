use std::{fmt, rc::Rc};

use super::primitive::InterpValue;

#[derive(Clone)]
pub struct ArrayValue(Rc<Vec<InterpValue>>);

impl ArrayValue {
    pub(crate) fn shared_capture_identity(&self) -> Option<*const ()> {
        (Rc::strong_count(&self.0) > 1).then(|| Rc::as_ptr(&self.0).cast())
    }

    pub(super) fn into_unique_values(self) -> Option<Vec<InterpValue>> {
        Rc::try_unwrap(self.0).ok()
    }

    pub fn new(values: Vec<InterpValue>) -> Self {
        Self(Rc::new(values))
    }

    pub fn borrow(&self) -> &Vec<InterpValue> {
        &self.0
    }

    pub fn borrow_mut(&mut self) -> &mut Vec<InterpValue> {
        Rc::make_mut(&mut self.0)
    }

    pub fn snapshot(&self) -> Vec<InterpValue> {
        self.borrow().clone()
    }

    /// Reuse unique backing; shared values retain their original snapshot.
    pub fn into_values(self) -> Vec<InterpValue> {
        match Rc::try_unwrap(self.0) {
            Ok(values) => values,
            Err(shared) => shared.as_ref().clone(),
        }
    }

    pub(crate) fn push(&mut self, value: InterpValue) {
        self.prepare_growth(1);
        Rc::make_mut(&mut self.0).push(value);
    }

    fn prepare_growth(&mut self, additional: usize) {
        if Rc::strong_count(&self.0) > 1 {
            let current = self.0.as_ref();
            // Clone shared headers directly into the final capacity, rather
            // than cloning a full buffer and immediately growing it again.
            let mut next = Vec::with_capacity(current.len() + additional);
            next.extend(current.iter().cloned());
            self.0 = Rc::new(next);
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
        let values = Rc::make_mut(&mut self.0);
        values.reserve(right_len);
        match Rc::try_unwrap(other.0) {
            Ok(right) => values.extend(right),
            Err(shared) => values.extend(shared.iter().cloned()),
        }
        self
    }

    pub fn make_unique(&mut self) {
        if Rc::strong_count(&self.0) > 1 {
            self.0 = Rc::new(self.snapshot());
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
    backing: Rc<Vec<InterpValue>>,
    range: std::ops::Range<usize>,
}

impl SliceValue {
    pub(crate) fn shared_capture_identity(&self) -> Option<(*const (), usize, usize)> {
        (Rc::strong_count(&self.backing) > 1).then(|| {
            (
                Rc::as_ptr(&self.backing).cast(),
                self.range.start,
                self.range.end,
            )
        })
    }

    pub(super) fn into_unique_backing(self) -> Option<Vec<InterpValue>> {
        Rc::try_unwrap(self.backing).ok()
    }

    pub fn new(values: Vec<InterpValue>) -> Self {
        Self {
            range: 0..values.len(),
            backing: Rc::new(values),
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

    pub fn borrow(&self) -> &[InterpValue] {
        &self.backing[self.range.clone()]
    }

    pub fn snapshot(&self) -> Vec<InterpValue> {
        self.borrow().to_vec()
    }

    /// Reuse unique backing; shared values retain their original snapshot.
    pub fn into_values(self) -> Vec<InterpValue> {
        match Rc::try_unwrap(self.backing) {
            Ok(mut values) => {
                values.truncate(self.range.end);
                values.drain(..self.range.start);
                values
            }
            Err(shared) => shared[self.range].to_vec(),
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

use std::{fmt, rc::Rc};

use super::primitive::InterpValue;

pub(crate) fn try_map_cloned<T: Clone, U, E>(
    values: &[T],
    mut map: impl FnMut(T) -> Result<U, E>,
) -> Result<Vec<U>, E> {
    let mut output = Vec::with_capacity(values.len());
    for value in values {
        output.push(map(value.clone())?);
    }
    Ok(output)
}

#[derive(Clone)]
pub struct ArrayValue {
    values: Rc<Vec<InterpValue>>,
    projected_from: Option<(std::rc::Weak<Vec<InterpValue>>, etas_types::TypeId)>,
}

impl ArrayValue {
    pub(crate) fn shared_capture_identity(&self) -> Option<*const ()> {
        (Rc::strong_count(&self.values) > 1).then(|| Rc::as_ptr(&self.values).cast())
    }

    pub(super) fn into_unique_values(self) -> Option<Vec<InterpValue>> {
        Rc::try_unwrap(self.values).ok()
    }

    pub fn new(values: Vec<InterpValue>) -> Self {
        Self {
            values: Rc::new(values),
            projected_from: None,
        }
    }

    pub fn borrow(&self) -> &Vec<InterpValue> {
        &self.values
    }

    pub fn borrow_mut(&mut self) -> &mut Vec<InterpValue> {
        self.projected_from = None;
        Rc::make_mut(&mut self.values)
    }

    pub fn snapshot(&self) -> Vec<InterpValue> {
        self.borrow().clone()
    }

    /// Reuse unique backing; shared values retain their original snapshot.
    pub fn into_values(self) -> Vec<InterpValue> {
        match Rc::try_unwrap(self.values) {
            Ok(values) => values,
            Err(shared) => shared.as_ref().clone(),
        }
    }

    /// Project shared elements directly into the destination, not a cloned runtime Vec.
    pub(crate) fn try_map_values<T, E>(
        self,
        map: impl FnMut(InterpValue) -> Result<T, E>,
    ) -> Result<Vec<T>, E> {
        match Rc::try_unwrap(self.values) {
            Ok(values) => values.into_iter().map(map).collect(),
            Err(shared) => try_map_cloned(&shared, map),
        }
    }

    pub(crate) fn push(&mut self, value: InterpValue) {
        self.prepare_growth(1);
        Rc::make_mut(&mut self.values).push(value);
    }

    fn prepare_growth(&mut self, additional: usize) {
        self.projected_from = None;
        if Rc::strong_count(&self.values) > 1 {
            let current = self.values.as_ref();
            // Clone shared headers directly into the final capacity, rather
            // than cloning a full buffer and immediately growing it again.
            let mut next = Vec::with_capacity(current.len() + additional);
            next.extend(current.iter().cloned());
            self.values = Rc::new(next);
        }
    }

    pub(crate) fn truncate(mut self, length: usize) -> Self {
        if length < self.borrow().len() {
            self.projected_from = None;
            if let Some(values) = Rc::get_mut(&mut self.values) {
                values.truncate(length);
            } else {
                return Self::new(self.borrow()[..length].to_vec());
            }
        }
        self
    }

    // Revision projection rebuilds typed elements but preserves their slot order.
    // Retain the source snapshot's identity without retaining its elements.
    pub(crate) fn index_origin(
        &self,
    ) -> (std::rc::Weak<Vec<InterpValue>>, Option<etas_types::TypeId>) {
        match &self.projected_from {
            Some((source, ty)) => (source.clone(), Some(*ty)),
            None => (Rc::downgrade(&self.values), None),
        }
    }

    pub(crate) fn projected(
        values: Vec<InterpValue>,
        source: std::rc::Weak<Vec<InterpValue>>,
        ty: etas_types::TypeId,
    ) -> Self {
        Self {
            values: Rc::new(values),
            projected_from: Some((source, ty)),
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
        let values = Rc::make_mut(&mut self.values);
        values.reserve(right_len);
        match Rc::try_unwrap(other.values) {
            Ok(right) => values.extend(right),
            Err(shared) => values.extend(shared.iter().cloned()),
        }
        self
    }

    pub fn make_unique(&mut self) {
        if Rc::strong_count(&self.values) > 1 {
            self.values = Rc::new(self.snapshot());
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
        super::comparison::value_equal(
            &InterpValue::Array(self.clone()),
            &InterpValue::Array(other.clone()),
        )
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
            backing: values.values,
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

    pub(crate) fn try_map_values<T, E>(
        self,
        map: impl FnMut(InterpValue) -> Result<T, E>,
    ) -> Result<Vec<T>, E> {
        match Rc::try_unwrap(self.backing) {
            Ok(mut values) => {
                values.truncate(self.range.end);
                values.drain(..self.range.start);
                values.into_iter().map(map).collect()
            }
            Err(shared) => try_map_cloned(&shared[self.range], map),
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
        super::comparison::value_equal(
            &InterpValue::Slice(self.clone()),
            &InterpValue::Slice(other.clone()),
        )
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

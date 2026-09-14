use std::{collections::VecDeque, rc::Rc};

use super::InterpValue;

#[derive(Clone, Debug)]
pub struct DequeValue(Rc<VecDeque<InterpValue>>);

impl DequeValue {
    pub(super) fn into_unique_values(self) -> Option<VecDeque<InterpValue>> {
        Rc::try_unwrap(self.0).ok()
    }

    pub fn new(values: Vec<InterpValue>) -> Self {
        Self(Rc::new(VecDeque::from(values)))
    }

    pub fn borrow(&self) -> &VecDeque<InterpValue> {
        &self.0
    }

    pub fn push_front(&mut self, value: InterpValue) {
        Rc::make_mut(&mut self.0).push_front(value);
    }

    pub fn push_back(&mut self, value: InterpValue) {
        Rc::make_mut(&mut self.0).push_back(value);
    }

    pub fn pop_front(&mut self) -> Option<InterpValue> {
        Rc::make_mut(&mut self.0).pop_front()
    }

    pub fn pop_back(&mut self) -> Option<InterpValue> {
        Rc::make_mut(&mut self.0).pop_back()
    }
}

impl From<Vec<InterpValue>> for DequeValue {
    fn from(values: Vec<InterpValue>) -> Self {
        Self::new(values)
    }
}

impl PartialEq for DequeValue {
    fn eq(&self, other: &Self) -> bool {
        self.borrow() == other.borrow()
    }
}

impl Eq for DequeValue {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        orchestration::ValueSnapshot, testing::allocation::measure,
        value::iteration::IterationSource,
    };

    #[test]
    fn unique_ring_end_operations_do_not_shift_or_copy_payloads() {
        for count in [1000, 2000, 4000] {
            let mut values = DequeValue::new(
                (0..count)
                    .map(|i| InterpValue::String(format!("{i:04}{}", "x".repeat(128)).into()))
                    .collect(),
            );
            let capacity = values.borrow().capacity();
            let second_address = &values.borrow()[1] as *const InterpValue;
            let (first, allocations) = measure(|| values.pop_front().unwrap());
            assert_eq!(allocations.count, 0);
            assert_eq!(
                &values.borrow()[0] as *const InterpValue,
                second_address,
                "pop_front must not shift remaining entries"
            );
            let (_, allocations) = measure(|| values.push_back(first));
            assert_eq!(allocations.count, 0);
            assert!(
                !values.borrow().as_slices().1.is_empty(),
                "exercise wrapped storage"
            );
            let (_, allocations) = measure(|| {
                for _ in 0..count {
                    let last = values.pop_back().unwrap();
                    values.push_front(last);
                }
            });
            assert_eq!(allocations.count, 0);
            assert_eq!(
                values.borrow().capacity(),
                capacity,
                "no retained-buffer growth during rotation"
            );
            let captured = ValueSnapshot::capture(&InterpValue::Deque(values.clone())).unwrap();
            assert_eq!(
                captured.restore().unwrap(),
                InterpValue::Deque(values.clone())
            );
            let (iterator, allocations) =
                measure(|| IterationSource::new(InterpValue::Queue(values.clone())).unwrap());
            assert_eq!(allocations.count, 0);
            let (_, allocations) = measure(|| values.pop_front().unwrap());
            assert_eq!(allocations.count, 2, "COW ring and backing; text is shared");
            assert!(allocations.bytes >= count * std::mem::size_of::<InterpValue>());
            assert_eq!(values.borrow().len(), count - 1);
            let (first, allocations) = measure(|| iterator.get(0).unwrap().unwrap());
            assert_eq!(allocations.count, 0, "selected text shares its backing");
            assert!(matches!(first, InterpValue::String(s) if s.starts_with("0001")));
        }
    }

    #[test]
    fn empty_ring_operations_and_value_equality_preserve_semantics() {
        let mut values = DequeValue::new(vec![]);
        assert_eq!(values.pop_front(), None);
        assert_eq!(values.pop_back(), None);
        values.push_back(InterpValue::i32(2));
        values.push_front(InterpValue::i32(1));
        let alias = values.clone();
        assert_eq!(
            values,
            DequeValue::new(vec![InterpValue::i32(1), InterpValue::i32(2)])
        );
        assert_eq!(values.pop_back(), Some(InterpValue::i32(2)));
        assert_eq!(alias.borrow().len(), 2);
        assert_eq!(values.borrow().len(), 1);
    }
}

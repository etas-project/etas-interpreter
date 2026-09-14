use std::cell::RefCell;

use super::{InterpValue, ListValue, range::IntegerRange};

/// Retains the evaluated collection version. Source updates use COW, so advancing the
/// cursor clones only the visited element, never the complete collection.
#[derive(Clone, Debug)]
pub(crate) struct IterationSource {
    value: InterpValue,
    list_cursor: RefCell<Option<(usize, ListValue)>>,
}

impl IterationSource {
    pub(crate) fn new(value: InterpValue) -> Result<Self, String> {
        match &value {
            InterpValue::Array(_)
            | InterpValue::List(_)
            | InterpValue::Slice(_)
            | InterpValue::Set(_)
            | InterpValue::Deque(_)
            | InterpValue::Queue(_)
            | InterpValue::Stack(_)
            | InterpValue::Map(_)
            | InterpValue::PriorityQueue(_)
            | InterpValue::OrderedMap(_)
            | InterpValue::OrderedSet(_) => {}
            InterpValue::Range(range) => {
                IntegerRange::from_value(range)
                    .map_err(|e| format!("invalid checked integer range: {e:?}"))?;
            }
            _ => return Err("for iteration requires a local collection or range value".into()),
        }
        Ok(Self {
            value,
            list_cursor: RefCell::new(None),
        })
    }

    pub(crate) fn value(&self) -> &InterpValue {
        &self.value
    }

    pub(crate) fn get(&self, index: usize) -> Result<Option<InterpValue>, String> {
        Ok(match &self.value {
            InterpValue::Array(values) | InterpValue::Stack(values) => {
                values.borrow().get(index).cloned()
            }
            InterpValue::Deque(values) | InterpValue::Queue(values) => {
                values.borrow().get(index).cloned()
            }
            InterpValue::List(values) => {
                let mut cursor = self.list_cursor.borrow_mut();
                if cursor
                    .as_ref()
                    .is_some_and(|(position, _)| *position > index)
                {
                    *cursor = None;
                }
                let (position, remaining) = cursor.get_or_insert_with(|| (0, values.clone()));
                while *position < index {
                    if !remaining.advance() {
                        return Ok(None);
                    }
                    *position += 1;
                }
                remaining.get(0).cloned()
            }
            InterpValue::Slice(values) => values.borrow().get(index).cloned(),
            InterpValue::Set(values) | InterpValue::OrderedSet(values) => {
                values.borrow().get(index).cloned()
            }
            InterpValue::Map(values)
            | InterpValue::PriorityQueue(values)
            | InterpValue::OrderedMap(values) => values
                .borrow()
                .get(index)
                .map(|(key, value)| InterpValue::Tuple(vec![key.clone(), value.clone()].into())),
            InterpValue::Range(range) => IntegerRange::from_value(range)
                .and_then(|range| range.get(index))
                .map_err(|e| format!("invalid checked integer range: {e:?}"))?
                .map(InterpValue::Number),
            _ => return Err("invalid iteration source".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::allocation::measure;
    use crate::value::{ArrayValue, NumericValue, RangeBounds, RangeValue};

    #[test]
    fn iteration_setup_and_early_break_do_not_copy_unvisited_payloads() {
        for count in [1000, 2000, 4000] {
            let values = ArrayValue::new(
                (0..count)
                    .map(|_| InterpValue::String("x".repeat(128).into()))
                    .collect(),
            );
            let input = InterpValue::Array(values.clone());
            let (source, setup) = measure(|| IterationSource::new(input).unwrap());
            assert_eq!(setup.count, 0);
            let (first, access) = measure(|| source.get(0).unwrap());
            assert!(matches!(first, Some(InterpValue::String(_))));
            assert_eq!(access.count, 0);
            assert_eq!(access.bytes, 0);
            let (_, drop_cost) = measure(|| drop(source));
            assert_eq!(drop_cost.count, 0);
        }
    }

    #[test]
    fn iterator_retains_collection_version_across_cow_updates() {
        let mut values = ArrayValue::new(vec![InterpValue::i32(1), InterpValue::i32(2)]);
        let source = IterationSource::new(InterpValue::Array(values.clone())).unwrap();
        values.make_unique();
        values.borrow_mut()[1] = InterpValue::i32(9);
        assert_eq!(source.get(1).unwrap(), Some(InterpValue::i32(2)));
        assert_eq!(values.borrow()[1], InterpValue::i32(9));
    }

    #[test]
    fn huge_range_iteration_is_lazy_and_stops_at_inclusive_endpoint() {
        let source = IterationSource::new(InterpValue::Range(RangeValue {
            start: Box::new(InterpValue::Number(NumericValue::U128(0))),
            end: Box::new(InterpValue::Number(NumericValue::U128(u128::MAX))),
            bounds: RangeBounds::ClosedClosed,
        }))
        .unwrap();
        let (first, allocations) = measure(|| source.get(0).unwrap());
        assert_eq!(first, Some(InterpValue::Number(NumericValue::U128(0))));
        assert_eq!(allocations.count, 0);
        assert!(IterationSource::new(InterpValue::Bool(true)).is_err());
    }

    #[test]
    fn map_iteration_allocates_only_the_visited_pair_not_the_backing() {
        for count in [1000, 2000, 4000] {
            let values = crate::value::MapValue::new(
                (0..count)
                    .map(|n| {
                        (
                            InterpValue::String(format!("{n}{}", "k".repeat(1024)).into()),
                            InterpValue::Bytes(vec![7; 1024].into()),
                        )
                    })
                    .collect(),
            );
            let input = InterpValue::Map(values.clone());
            let (source, setup) = measure(|| IterationSource::new(input).unwrap());
            assert_eq!(setup.count, 0, "n={count}: {setup:?}");
            let (first, access) = measure(|| source.get(0).unwrap());
            assert!(matches!(first, Some(InterpValue::Tuple(_))));
            assert_eq!(access.count, 2, "n={count}: {access:?}");
            assert!(
                access.bytes <= 2 * std::mem::size_of::<InterpValue>() + 128,
                "n={count}: {access:?}"
            );
            let (_, drop_cost) = measure(|| drop(source));
            assert_eq!(drop_cost.count, 0);
        }
    }
}

use super::numeric::NumericError;
use super::{InterpValue, NumericValue, RangeBounds, RangeValue};

/// A finite integer interval normalized to inclusive endpoints, without computing its length.
/// The full u128 interval has 2^128 elements and cannot have a u128 length.
#[derive(Clone, Copy, Debug)]
pub(crate) struct IntegerRange {
    origin: NumericValue,
    first: NumericValue,
    last: NumericValue,
    empty: bool,
}

impl IntegerRange {
    pub(crate) fn new(
        start: NumericValue,
        end: NumericValue,
        bounds: RangeBounds,
    ) -> Result<Self, NumericError> {
        let one = start.one_same()?;
        end.one_same()?;
        let ordering = start
            .partial_cmp_same(end)?
            .ok_or(NumericError::TypeMismatch)?;
        let empty = ordering.is_gt() || (ordering.is_eq() && bounds != RangeBounds::ClosedClosed);
        let (first, last) = if empty {
            (start, start)
        } else {
            let first = if matches!(bounds, RangeBounds::OpenOpen | RangeBounds::OpenClosed) {
                start.checked_add(one)?
            } else {
                start
            };
            let last = if matches!(bounds, RangeBounds::OpenOpen | RangeBounds::ClosedOpen) {
                end.checked_sub(one)?
            } else {
                end
            };
            (first, last)
        };
        Ok(Self {
            origin: start,
            first,
            last,
            empty: empty
                || first
                    .partial_cmp_same(last)?
                    .is_some_and(|order| order.is_gt()),
        })
    }

    pub(crate) fn from_value(range: &RangeValue) -> Result<Self, NumericError> {
        match (&*range.start, &*range.end) {
            (InterpValue::Number(start), InterpValue::Number(end)) => {
                Self::new(*start, *end, range.bounds)
            }
            _ => Err(NumericError::TypeMismatch),
        }
    }

    pub(crate) fn get(self, index: usize) -> Result<Option<NumericValue>, NumericError> {
        if self.empty {
            return Ok(None);
        }
        let offset = index as u128;
        if let (Some(first), Some(last)) = (self.first.as_i128(), self.last.as_i128()) {
            if offset > last.abs_diff(first) {
                return Ok(None);
            }
            let value = first
                .checked_add_unsigned(offset)
                .ok_or(NumericError::Overflow)?;
            return NumericValue::from_signed(value, self.first.primitive())
                .map(Some)
                .ok_or(NumericError::Overflow);
        }
        if let (Some(first), Some(last)) = (self.first.as_u128(), self.last.as_u128()) {
            if offset > last - first {
                return Ok(None);
            }
            let value = first.checked_add(offset).ok_or(NumericError::Overflow)?;
            return NumericValue::from_unsigned(value, self.first.primitive())
                .map(Some)
                .ok_or(NumericError::Overflow);
        }
        Err(NumericError::TypeMismatch)
    }

    pub(crate) fn allows_position(self, position: usize) -> Result<bool, NumericError> {
        match position.checked_sub(1) {
            None => Ok(true),
            Some(previous) => Ok(self.get(previous)?.is_some()),
        }
    }

    pub(crate) fn slice(self, start: usize, end: usize) -> Result<RangeValue, NumericError> {
        if start > end || !self.allows_position(end)? {
            return Err(NumericError::Overflow);
        }
        if start == end {
            return Ok(RangeValue {
                start: Box::new(InterpValue::Number(self.origin)),
                end: Box::new(InterpValue::Number(self.origin)),
                bounds: RangeBounds::ClosedOpen,
            });
        }
        let first = self.get(start)?.ok_or(NumericError::Overflow)?;
        let last = self.get(end - 1)?.ok_or(NumericError::Overflow)?;
        // Keep an inclusive endpoint when the integer type cannot represent its successor.
        let (end, bounds) = match last.checked_add(last.one_same()?) {
            Ok(end) => (end, RangeBounds::ClosedOpen),
            Err(NumericError::Overflow) => (last, RangeBounds::ClosedClosed),
            Err(error) => return Err(error),
        };
        Ok(RangeValue {
            start: Box::new(InterpValue::Number(first)),
            end: Box::new(InterpValue::Number(end)),
            bounds,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::allocation::measure;
    use etas_types::PrimitiveType;

    #[test]
    fn arithmetic_range_agrees_with_finite_enumeration_for_all_bounds() {
        for start in -5..=5 {
            for end in -5..=5 {
                for bounds in [
                    RangeBounds::ClosedClosed,
                    RangeBounds::ClosedOpen,
                    RangeBounds::OpenClosed,
                    RangeBounds::OpenOpen,
                ] {
                    let expected: Vec<_> = (-5..=5)
                        .filter(|value| {
                            let lower = if matches!(
                                bounds,
                                RangeBounds::ClosedClosed | RangeBounds::ClosedOpen
                            ) {
                                *value >= start
                            } else {
                                *value > start
                            };
                            let upper = if matches!(
                                bounds,
                                RangeBounds::ClosedClosed | RangeBounds::OpenClosed
                            ) {
                                *value <= end
                            } else {
                                *value < end
                            };
                            lower && upper
                        })
                        .map(NumericValue::I32)
                        .collect();
                    let range =
                        IntegerRange::new(NumericValue::I32(start), NumericValue::I32(end), bounds)
                            .unwrap();
                    for index in 0..=expected.len() + 1 {
                        assert_eq!(range.get(index).unwrap(), expected.get(index).copied());
                        assert_eq!(
                            range.allows_position(index).unwrap(),
                            index <= expected.len()
                        );
                    }
                    for from in 0..=expected.len() {
                        for to in from..=expected.len() {
                            let sliced = range.slice(from, to).unwrap();
                            let sliced = IntegerRange::from_value(&sliced).unwrap();
                            for i in 0..=to - from {
                                assert_eq!(
                                    sliced.get(i).unwrap(),
                                    expected[from..to].get(i).copied()
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn range_preserves_integer_width_at_inclusive_maximum() {
        let maximums = [
            NumericValue::I8(i8::MAX),
            NumericValue::I16(i16::MAX),
            NumericValue::I32(i32::MAX),
            NumericValue::I64(i64::MAX),
            NumericValue::I128(i128::MAX),
            NumericValue::ISize(i64::MAX),
            NumericValue::U8(u8::MAX),
            NumericValue::U16(u16::MAX),
            NumericValue::U32(u32::MAX),
            NumericValue::U64(u64::MAX),
            NumericValue::U128(u128::MAX),
            NumericValue::USize(u64::MAX),
        ];
        for last in maximums {
            let first = last.checked_sub(last.one_same().unwrap()).unwrap();
            let range = IntegerRange::new(first, last, RangeBounds::ClosedClosed).unwrap();
            assert_eq!(range.get(0).unwrap(), Some(first));
            assert_eq!(range.get(1).unwrap(), Some(last));
            assert_eq!(range.get(2).unwrap(), None);
            let slice = range.slice(1, 2).unwrap();
            assert_eq!(
                IntegerRange::from_value(&slice).unwrap().get(0).unwrap(),
                Some(last)
            );
            assert_eq!(slice.bounds, RangeBounds::ClosedClosed);
        }
        assert!(
            IntegerRange::new(
                NumericValue::I32(1),
                NumericValue::U32(3),
                RangeBounds::ClosedOpen
            )
            .is_err()
        );
        assert!(
            IntegerRange::new(
                NumericValue::F32(0),
                NumericValue::F32(0),
                RangeBounds::ClosedOpen
            )
            .is_err()
        );
        assert!(
            IntegerRange::new(
                NumericValue::I32(0),
                NumericValue::I32(1),
                RangeBounds::ClosedOpen
            )
            .unwrap()
            .slice(0, 2)
            .is_err()
        );
    }

    #[test]
    fn tiny_slice_of_full_width_range_has_constant_auxiliary_storage() {
        for (start, end) in [
            (NumericValue::U128(0), NumericValue::U128(u128::MAX)),
            (NumericValue::I128(i128::MIN), NumericValue::I128(i128::MAX)),
        ] {
            let range = IntegerRange::new(start, end, RangeBounds::ClosedClosed).unwrap();
            let (slice, allocations) = measure(|| range.slice(3, 6).unwrap());
            assert_eq!(allocations.count, 2);
            assert_eq!(allocations.bytes, 2 * std::mem::size_of::<InterpValue>());
            let sliced = IntegerRange::from_value(&slice).unwrap();
            assert_eq!(sliced.get(0).unwrap(), range.get(3).unwrap());
            assert_eq!(sliced.get(3).unwrap(), None);
            assert!(range.get(usize::MAX).unwrap().is_some());
        }
        let zero = NumericValue::from_unsigned(0, PrimitiveType::U128).unwrap();
        assert!(
            IntegerRange::new(zero, zero, RangeBounds::OpenOpen)
                .unwrap()
                .get(0)
                .unwrap()
                .is_none()
        );
    }
}

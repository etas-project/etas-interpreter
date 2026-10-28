use super::*;

use crate::orchestration::ValueSnapshot;

impl DecodedValue for ValueSnapshot {
    type Frame = crate::orchestration::LocalsSnapshot;
    fn frame_cache(
        limits: &etas_host::StorageLimits,
        root: machine::DecodeRoot<'_>,
    ) -> Result<machine::FrameDecodeCache<Self::Frame>, String> {
        machine::snapshot_frame_cache(limits, root)
    }
    fn callable(
        limits: &etas_host::StorageLimits,
        value: &Value,
        cache: &machine::FrameDecodeCache<Self::Frame>,
    ) -> Result<Self, InterpreterCodecError> {
        machine::call_target_snapshot_with_frames(limits, value, cache)
            .map(Self::Callable)
            .map_err(InterpreterCodecError::new)
    }
    fn scalar(
        limits: &etas_host::StorageLimits,
        value: &Value,
    ) -> Result<Self, InterpreterCodecError> {
        super::super::snapshot_support::decode(limits, value)
    }
    fn unary(kind: Unary, value: Self) -> Self {
        let value = crate::orchestration::SnapshotBox::new(value);
        match kind {
            Unary::Nominal(ty) => Self::Nominal { ty, value },
            Unary::Trust(wrapper) => Self::Trust { wrapper, value },
            Unary::Some => Self::OptionSome(value),
        }
    }
    fn sequence(
        kind: Sequence,
        values: Vec<Self>,
        comparator: Option<Self>,
    ) -> Result<Self, InterpreterCodecError> {
        validate_comparison(comparator.as_ref())?;
        if matches!(kind, Sequence::Set | Sequence::OrderedSet) {
            crate::value::membership::MembershipIndex::require_unique(&values)
                .map_err(InterpreterCodecError::new)?;
        }
        Ok(match kind {
            Sequence::Tuple => Self::Tuple(values.into()),
            Sequence::Array => Self::Array(values.into()),
            Sequence::List => Self::List(values.into()),
            Sequence::Slice => Self::Slice(values.into()),
            Sequence::Set => Self::Set(values.into()),
            Sequence::Deque => Self::Deque(values.into()),
            Sequence::Queue => Self::Queue(values.into()),
            Sequence::Stack => Self::Stack(values.into()),
            Sequence::OrderedSet => Self::OrderedSet {
                values: values.into(),
                comparator: comparator.map(crate::orchestration::SnapshotBox::new),
            },
            Sequence::Variant(name) => Self::Variant {
                name,
                fields: values.into(),
            },
        })
    }
    fn pairs(
        kind: Pairs,
        values: Vec<(Self, Self)>,
        comparator: Option<Self>,
    ) -> Result<Self, InterpreterCodecError> {
        Ok(match kind {
            Pairs::Map => Self::Map(values.into()),
            Pairs::OrderedMap => {
                validate_comparison(comparator.as_ref())?;
                Self::OrderedMap {
                    entries: values.into(),
                    comparator: comparator.map(crate::orchestration::SnapshotBox::new),
                }
            }
            Pairs::PriorityQueue => {
                if comparator
                    .as_ref()
                    .is_some_and(|value| !matches!(value, Self::Callable(_)))
                {
                    return Err(InterpreterCodecError::new(
                        "priority comparison must be a callable",
                    ));
                }
                Self::PriorityQueue {
                    entries: values.into(),
                    comparator: comparator.map(crate::orchestration::SnapshotBox::new),
                }
            }
        })
    }
    fn record(values: Vec<(String, Self)>) -> Self {
        Self::Record(values.into())
    }
    fn range(start: Self, end: Self, bounds: crate::value::RangeBounds) -> Self {
        Self::Range {
            start: crate::orchestration::SnapshotBox::new(start),
            end: crate::orchestration::SnapshotBox::new(end),
            bounds,
        }
    }
}

fn validate_comparison(comparator: Option<&ValueSnapshot>) -> Result<(), InterpreterCodecError> {
    if comparator.is_some_and(|value| !matches!(value, ValueSnapshot::Callable(_))) {
        Err(InterpreterCodecError::new(
            "key comparison must be a callable",
        ))
    } else {
        Ok(())
    }
}

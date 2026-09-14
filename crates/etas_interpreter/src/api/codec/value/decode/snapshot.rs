use super::*;

use crate::orchestration::ValueSnapshot;

impl DecodedValue for ValueSnapshot {
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
    fn sequence(kind: Sequence, values: Vec<Self>) -> Result<Self, InterpreterCodecError> {
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
            Sequence::OrderedSet => Self::OrderedSet(values.into()),
            Sequence::Variant(name) => Self::Variant {
                name,
                fields: values.into(),
            },
        })
    }
    fn pairs(kind: Pairs, values: Vec<(Self, Self)>) -> Self {
        match kind {
            Pairs::Map => Self::Map(values.into()),
            Pairs::OrderedMap => Self::OrderedMap(values.into()),
            Pairs::PriorityQueue => Self::PriorityQueue(values.into()),
        }
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

use super::*;

impl DecodedValue for InterpValue {
    fn scalar(
        limits: &etas_host::StorageLimits,
        value: &Value,
    ) -> Result<Self, InterpreterCodecError> {
        decode_scalar_or_support(limits, value)
    }
    fn unary(kind: Unary, value: Self) -> Self {
        let value = value.into();
        match kind {
            Unary::Nominal(ty) => Self::Nominal { ty, value },
            Unary::Trust(wrapper) => Self::Trust { wrapper, value },
            Unary::Some => Self::OptionSome(value),
        }
    }
    fn sequence(kind: Sequence, values: Vec<Self>) -> Result<Self, InterpreterCodecError> {
        Ok(match kind {
            Sequence::Tuple => Self::Tuple(values.into()),
            Sequence::Array => Self::Array(values.into()),
            Sequence::List => Self::List(values.into()),
            Sequence::Slice => Self::Slice(values.into()),
            Sequence::Set => Self::Set(
                crate::value::SetValue::from_unique(values).map_err(InterpreterCodecError::new)?,
            ),
            Sequence::Deque => Self::Deque(values.into()),
            Sequence::Queue => Self::Queue(values.into()),
            Sequence::Stack => Self::Stack(values.into()),
            Sequence::OrderedSet => Self::OrderedSet(
                crate::value::SetValue::from_unique(values).map_err(InterpreterCodecError::new)?,
            ),
            Sequence::Variant(name) => Self::Variant {
                name: name.into(),
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
        Self::Range(crate::value::RangeValue {
            start: Box::new(start),
            end: Box::new(end),
            bounds,
        })
    }
}

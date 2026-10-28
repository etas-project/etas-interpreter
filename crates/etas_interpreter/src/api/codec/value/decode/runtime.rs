use super::*;

impl DecodedValue for InterpValue {
    type Frame = crate::control::Frame;
    fn frame_cache(
        limits: &etas_host::StorageLimits,
        root: machine::DecodeRoot<'_>,
    ) -> Result<machine::FrameDecodeCache<Self::Frame>, String> {
        machine::runtime_frame_cache(limits, root)
    }
    fn callable(
        limits: &etas_host::StorageLimits,
        value: &Value,
        cache: &machine::FrameDecodeCache<Self::Frame>,
    ) -> Result<Self, InterpreterCodecError> {
        machine::call_target_with_frames(limits, value, cache)
            .map(Self::Callable)
            .map_err(InterpreterCodecError::new)
    }
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
    fn sequence(
        kind: Sequence,
        values: Vec<Self>,
        comparator: Option<Self>,
    ) -> Result<Self, InterpreterCodecError> {
        if comparator.is_some() {
            return Err(InterpreterCodecError::new(
                "restoring executable key comparisons requires a checked checkpoint context",
            ));
        }
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
                crate::value::OrderedSetValue::try_from_values(values)
                    .map_err(InterpreterCodecError::new)?,
            ),
            Sequence::Variant(name) => Self::Variant {
                name: name.into(),
                fields: values.into(),
            },
        })
    }
    fn pairs(
        kind: Pairs,
        values: Vec<(Self, Self)>,
        comparator: Option<Self>,
    ) -> Result<Self, InterpreterCodecError> {
        if matches!(kind, Pairs::OrderedMap) && comparator.is_some() {
            return Err(InterpreterCodecError::new(
                "restoring executable key comparisons requires a checked checkpoint context",
            ));
        }
        Ok(match kind {
            Pairs::Map => Self::Map(values.into()),
            Pairs::OrderedMap => Self::OrderedMap(
                crate::value::OrderedMapValue::try_from_entries(values)
                    .map_err(InterpreterCodecError::new)?,
            ),
            Pairs::PriorityQueue => {
                let comparator = comparator
                    .map(|value| match value {
                        Self::Callable(target) => Ok(target),
                        _ => Err(InterpreterCodecError::new(
                            "priority comparison must be a callable",
                        )),
                    })
                    .transpose()?;
                Self::PriorityQueue(crate::value::PriorityQueueValue::from_entries(
                    values, comparator,
                ))
            }
        })
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

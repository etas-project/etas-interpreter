use super::*;

pub(super) mod host;
pub(super) mod json;

#[cfg(test)]
mod tests;

pub(super) fn decode<T: DecodedValue>(
    limits: &etas_host::StorageLimits,
    root: &Value,
) -> Result<T, InterpreterCodecError> {
    let mut frames = Vec::new();
    let mut next = root;
    loop {
        let mut result = if let Some(frame) = Frame::for_value(next)? {
            if let Some(child) = frame.next_child()? {
                next = child;
                frames.push(frame);
                continue;
            }
            frame.finish()?
        } else {
            T::scalar(limits, next)?
        };
        loop {
            let Some(mut frame) = frames.pop() else {
                return Ok(result);
            };
            frame.accept(result)?;
            if let Some(child) = frame.next_child()? {
                next = child;
                frames.push(frame);
                break;
            }
            result = frame.finish()?;
        }
    }
}

pub(super) enum Unary {
    Nominal(TypeId),
    Trust(etas_types::TrustWrapper),
    Some,
}

pub(super) enum Sequence {
    Tuple,
    Array,
    List,
    Slice,
    Set,
    Deque,
    Queue,
    Stack,
    OrderedSet,
    Variant(String),
}

pub(super) enum Pairs {
    Map,
    OrderedMap,
    PriorityQueue,
}

enum Frame<'a, T> {
    Unary {
        kind: Unary,
        input: &'a Value,
        output: Option<T>,
    },
    Sequence {
        kind: Sequence,
        input: &'a [Value],
        output: Vec<T>,
    },
    Record {
        input: &'a [Value],
        output: Vec<(String, T)>,
    },
    Pairs {
        kind: Pairs,
        input: &'a [Value],
        output: Vec<(T, T)>,
        key: Option<T>,
    },
    Range {
        input: &'a Value,
        start: Option<T>,
        end: Option<T>,
    },
}

impl<'a, T: DecodedValue> Frame<'a, T> {
    fn for_value(value: &'a Value) -> Result<Option<Self>, InterpreterCodecError> {
        let kind = required_str(value, "kind")?;
        let unary = match kind {
            "nominal" => Some(Unary::Nominal(TypeId(required_u32(value, "ty")?))),
            "trust" => Some(Unary::Trust(
                value_codec::trust_wrapper_from_json(required_str(value, "wrapper")?)
                    .map_err(InterpreterCodecError::new)?,
            )),
            "option_some" => Some(Unary::Some),
            _ => None,
        };
        if let Some(kind) = unary {
            return Ok(Some(Self::Unary {
                kind,
                input: required_obj(value, "value")?,
                output: None,
            }));
        }
        let sequence = match kind {
            "tuple" => Some(Sequence::Tuple),
            "array" => Some(Sequence::Array),
            "list" => Some(Sequence::List),
            "slice" => Some(Sequence::Slice),
            "set" => Some(Sequence::Set),
            "deque" => Some(Sequence::Deque),
            "queue" => Some(Sequence::Queue),
            "stack" => Some(Sequence::Stack),
            "ordered_set" => Some(Sequence::OrderedSet),
            "variant" => Some(Sequence::Variant(required_str(value, "name")?.to_owned())),
            _ => None,
        };
        if let Some(kind) = sequence {
            let input = required_array(
                value,
                if matches!(kind, Sequence::Variant(_)) {
                    "fields"
                } else {
                    "values"
                },
            )?;
            return Ok(Some(Self::Sequence {
                kind,
                input,
                output: Vec::with_capacity(input.len()),
            }));
        }
        let pairs = match kind {
            "map" => Some(Pairs::Map),
            "ordered_map" => Some(Pairs::OrderedMap),
            "priority_queue" => Some(Pairs::PriorityQueue),
            _ => None,
        };
        if let Some(kind) = pairs {
            let input = required_array(value, "entries")?;
            return Ok(Some(Self::Pairs {
                kind,
                input,
                output: Vec::with_capacity(input.len()),
                key: None,
            }));
        }
        Ok(match kind {
            "record" => {
                let input = required_array(value, "fields")?;
                Some(Self::Record {
                    input,
                    output: Vec::with_capacity(input.len()),
                })
            }
            "range" => Some(Self::Range {
                input: value,
                start: None,
                end: None,
            }),
            _ => None,
        })
    }

    fn next_child(&self) -> Result<Option<&'a Value>, InterpreterCodecError> {
        match self {
            Self::Unary { input, output, .. } => Ok(output.is_none().then_some(*input)),
            Self::Sequence { input, output, .. } => Ok(input.get(output.len())),
            Self::Record { input, output } => input
                .get(output.len())
                .map(|field| {
                    required_str(field, "name")?;
                    required_obj(field, "value")
                })
                .transpose(),
            Self::Pairs {
                kind,
                input,
                output,
                key,
            } => input
                .get(output.len())
                .map(|entry| {
                    required_obj(
                        entry,
                        if key.is_some() {
                            "value"
                        } else if matches!(kind, Pairs::PriorityQueue) {
                            "priority"
                        } else {
                            "key"
                        },
                    )
                })
                .transpose(),
            Self::Range { input, start, end } => {
                if start.is_none() {
                    required_obj(input, "start").map(Some)
                } else if end.is_none() {
                    required_obj(input, "end").map(Some)
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn accept(&mut self, value: T) -> Result<(), InterpreterCodecError> {
        match self {
            Self::Unary { output, .. } => *output = Some(value),
            Self::Sequence { output, .. } => output.push(value),
            Self::Record { input, output } => {
                let field = input.get(output.len()).ok_or_else(invalid_builder)?;
                output.push((required_str(field, "name")?.to_owned(), value));
            }
            Self::Pairs { output, key, .. } => {
                if let Some(key) = key.take() {
                    output.push((key, value));
                } else {
                    *key = Some(value);
                }
            }
            Self::Range { start, end, .. } => {
                if start.is_none() {
                    *start = Some(value);
                } else {
                    *end = Some(value);
                }
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<T, InterpreterCodecError> {
        Ok(match self {
            Self::Unary { kind, output, .. } => T::unary(kind, output.ok_or_else(invalid_builder)?),
            Self::Sequence { kind, output, .. } => T::sequence(kind, output)?,
            Self::Record { output, .. } => T::record(output),
            Self::Pairs {
                kind, output, key, ..
            } => {
                if key.is_some() {
                    return Err(invalid_builder());
                }
                T::pairs(kind, output)
            }
            Self::Range { input, start, end } => T::range(
                start.ok_or_else(invalid_builder)?,
                end.ok_or_else(invalid_builder)?,
                value_codec::range_bounds_from_json(required_str(input, "bounds")?)
                    .map_err(InterpreterCodecError::new)?,
            ),
        })
    }
}

pub(super) trait DecodedValue: Sized {
    fn scalar(
        limits: &etas_host::StorageLimits,
        value: &Value,
    ) -> Result<Self, InterpreterCodecError>;
    fn unary(kind: Unary, value: Self) -> Self;
    fn sequence(kind: Sequence, values: Vec<Self>) -> Result<Self, InterpreterCodecError>;
    fn pairs(kind: Pairs, values: Vec<(Self, Self)>) -> Self;
    fn record(values: Vec<(String, Self)>) -> Self;
    fn range(start: Self, end: Self, bounds: crate::value::RangeBounds) -> Self;
}

mod runtime;
mod snapshot;

fn invalid_builder() -> InterpreterCodecError {
    InterpreterCodecError::new("invalid aggregate value decoder state")
}

use etas_builtin::{
    BuiltinError, BuiltinRangeBounds, BuiltinValue, call_pure_intrinsic, collections,
};
use etas_std::{StdIntrinsicId, intrinsic};

use crate::value::{
    ArrayValue, InterpValue, ListValue, MapValue, RangeBounds, RangeValue, RecordValue, SliceValue,
};

#[derive(Clone, Debug, PartialEq)]
pub enum AdapterError {
    Builtin(BuiltinError),
    UnsupportedValue(String),
}

pub fn execute_pure_intrinsic(
    intrinsic: StdIntrinsicId,
    args: Vec<InterpValue>,
) -> Result<InterpValue, AdapterError> {
    if let Some(value) = interpreter_fast_path(intrinsic, &args)? {
        return Ok(value);
    }
    let builtin_args = into_builtin_args(intrinsic, args)?;
    let result = call_pure_intrinsic(intrinsic, &builtin_args).map_err(AdapterError::Builtin)?;
    from_builtin(result)
}

fn interpreter_fast_path(
    intrinsic: StdIntrinsicId,
    args: &[InterpValue],
) -> Result<Option<InterpValue>, AdapterError> {
    match (intrinsic.0, args) {
        (intrinsic::pure::OPTION_IS_SOME, [value]) => Ok(Some(InterpValue::Bool(matches!(
            value,
            InterpValue::OptionSome(_)
        )))),
        (intrinsic::pure::OPTION_IS_NONE, [value]) => Ok(Some(InterpValue::Bool(matches!(
            value,
            InterpValue::OptionNone
        )))),
        (intrinsic::pure::OPTION_UNWRAP, [InterpValue::OptionSome(value)]) => {
            Ok(Some((**value).clone()))
        }
        (intrinsic::pure::OPTION_UNWRAP, [InterpValue::Variant { name, fields }])
            if name == "Ok" && fields.len() == 1 =>
        {
            Ok(Some(fields.first().expect("ok field").clone()))
        }
        (intrinsic::pure::LIST_LEN, [InterpValue::Array(values)]) => Ok(Some(from_builtin(
            collections::list::len_from_count(values.borrow().len()),
        )?)),
        (intrinsic::pure::LIST_LEN, [InterpValue::List(values)]) => Ok(Some(from_builtin(
            collections::list::len_from_count(values.borrow().len()),
        )?)),
        (intrinsic::pure::LIST_LEN, [InterpValue::Slice(values)]) => Ok(Some(from_builtin(
            collections::list::len_from_count(values.borrow().len()),
        )?)),
        (intrinsic::pure::LIST_LEN, [InterpValue::Deque(values)])
        | (intrinsic::pure::LIST_LEN, [InterpValue::Queue(values)])
        | (intrinsic::pure::LIST_LEN, [InterpValue::Stack(values)]) => Ok(Some(from_builtin(
            collections::list::len_from_count(values.borrow().len()),
        )?)),
        (intrinsic::pure::LIST_LEN, [InterpValue::PriorityQueue(entries)])
        | (intrinsic::pure::LIST_LEN, [InterpValue::OrderedMap(entries)]) => Ok(Some(
            from_builtin(collections::list::len_from_count(entries.borrow().len()))?,
        )),
        (intrinsic::pure::LIST_LEN, [InterpValue::OrderedSet(values)]) => Ok(Some(from_builtin(
            collections::list::len_from_count(values.borrow().len()),
        )?)),
        (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::Array(values)]) => Ok(Some(from_builtin(
            collections::list::is_empty_from_count(values.borrow().len()),
        )?)),
        (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::List(values)]) => Ok(Some(from_builtin(
            collections::list::is_empty_from_count(values.borrow().len()),
        )?)),
        (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::Slice(values)]) => Ok(Some(from_builtin(
            collections::list::is_empty_from_count(values.borrow().len()),
        )?)),
        (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::Deque(values)])
        | (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::Queue(values)])
        | (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::Stack(values)]) => Ok(Some(from_builtin(
            collections::list::is_empty_from_count(values.borrow().len()),
        )?)),
        (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::PriorityQueue(entries)])
        | (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::OrderedMap(entries)]) => {
            Ok(Some(from_builtin(collections::list::is_empty_from_count(
                entries.borrow().len(),
            ))?))
        }
        (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::OrderedSet(values)]) => {
            Ok(Some(from_builtin(collections::list::is_empty_from_count(
                values.borrow().len(),
            ))?))
        }
        _ => Ok(None),
    }
}

fn into_builtin_args(
    intrinsic: StdIntrinsicId,
    args: Vec<InterpValue>,
) -> Result<Vec<BuiltinValue>, AdapterError> {
    let _ = intrinsic;
    args.into_iter()
        .map(into_builtin)
        .collect::<Result<Vec<_>, AdapterError>>()
}

fn into_builtin(value: InterpValue) -> Result<BuiltinValue, AdapterError> {
    match value {
        InterpValue::Unit => Ok(BuiltinValue::Unit),
        InterpValue::Bool(value) => Ok(BuiltinValue::Bool(value)),
        InterpValue::Number(crate::value::NumericValue::I32(value)) => Ok(BuiltinValue::I32(value)),
        InterpValue::Number(crate::value::NumericValue::I64(value)) => Ok(BuiltinValue::I64(value)),
        InterpValue::Number(crate::value::NumericValue::USize(value)) => usize::try_from(value)
            .map(BuiltinValue::Usize)
            .map_err(|_| AdapterError::UnsupportedValue(format!("usize value {value}"))),
        InterpValue::String(value) => Ok(BuiltinValue::String(value)),
        InterpValue::Bytes(value) => Ok(BuiltinValue::Bytes(value)),
        InterpValue::Array(values) => values
            .snapshot()
            .into_iter()
            .map(into_builtin)
            .collect::<Result<Vec<_>, _>>()
            .map(BuiltinValue::Array),
        InterpValue::List(values) => values
            .snapshot()
            .into_iter()
            .map(into_builtin)
            .collect::<Result<Vec<_>, _>>()
            .map(BuiltinValue::List),
        InterpValue::Slice(values) => values
            .snapshot()
            .into_iter()
            .map(into_builtin)
            .collect::<Result<Vec<_>, _>>()
            .map(BuiltinValue::Slice),
        InterpValue::Map(entries) => entries
            .snapshot()
            .into_iter()
            .map(|(key, value)| Ok((into_builtin(key)?, into_builtin(value)?)))
            .collect::<Result<Vec<_>, _>>()
            .map(BuiltinValue::Map),
        InterpValue::Record(fields) => fields
            .snapshot()
            .into_iter()
            .map(|(field, value)| Ok((field, into_builtin(value)?)))
            .collect::<Result<Vec<_>, _>>()
            .map(BuiltinValue::Record),
        InterpValue::Set(values) => Err(AdapterError::UnsupportedValue(format!(
            "set values are not supported by pure builtin adapter yet: {:?}",
            values.snapshot()
        ))),
        InterpValue::Deque(values) | InterpValue::Queue(values) | InterpValue::Stack(values) => {
            Err(AdapterError::UnsupportedValue(format!(
                "advanced sequence values are not supported by pure builtin adapter yet: {:?}",
                values.snapshot()
            )))
        }
        InterpValue::PriorityQueue(entries) | InterpValue::OrderedMap(entries) => {
            Err(AdapterError::UnsupportedValue(format!(
                "advanced map values are not supported by pure builtin adapter yet: {:?}",
                entries.snapshot()
            )))
        }
        InterpValue::OrderedSet(values) => Err(AdapterError::UnsupportedValue(format!(
            "ordered set values are not supported by pure builtin adapter yet: {:?}",
            values.snapshot()
        ))),
        InterpValue::Range(range) => Ok(BuiltinValue::Range {
            start: Box::new(into_builtin(*range.start)?),
            end: Box::new(into_builtin(*range.end)?),
            bounds: into_builtin_range_bounds(range.bounds),
        }),
        InterpValue::OptionNone => Ok(BuiltinValue::OptionNone),
        InterpValue::OptionSome(value) => into_builtin(*value)
            .map(Box::new)
            .map(BuiltinValue::OptionSome),
        InterpValue::Variant { name, fields } if name == "Ok" && fields.len() == 1 => {
            into_builtin(fields.into_iter().next().expect("ok field"))
                .map(Box::new)
                .map(BuiltinValue::ResultOk)
        }
        InterpValue::Variant { name, fields } if name == "Err" && fields.len() == 1 => {
            into_builtin(fields.into_iter().next().expect("err field"))
                .map(Box::new)
                .map(BuiltinValue::ResultErr)
        }
        InterpValue::Variant { name, fields } => fields
            .into_iter()
            .map(into_builtin)
            .collect::<Result<Vec<_>, _>>()
            .map(|fields| BuiltinValue::Variant { name, fields }),
        other => Err(AdapterError::UnsupportedValue(format!("{other:?}"))),
    }
}

fn from_builtin(value: BuiltinValue) -> Result<InterpValue, AdapterError> {
    match value {
        BuiltinValue::Unit => Ok(InterpValue::Unit),
        BuiltinValue::Bool(value) => Ok(InterpValue::Bool(value)),
        BuiltinValue::I32(value) => Ok(InterpValue::i32(value)),
        BuiltinValue::I64(value) => Ok(InterpValue::i64(value)),
        BuiltinValue::Usize(value) => Ok(InterpValue::usize(value)),
        BuiltinValue::String(value) => Ok(InterpValue::String(value)),
        BuiltinValue::Bytes(value) => Ok(InterpValue::Bytes(value)),
        BuiltinValue::Array(values) => values
            .into_iter()
            .map(from_builtin)
            .collect::<Result<Vec<_>, _>>()
            .map(ArrayValue::new)
            .map(InterpValue::Array),
        BuiltinValue::List(values) => values
            .into_iter()
            .map(from_builtin)
            .collect::<Result<Vec<_>, _>>()
            .map(ListValue::new)
            .map(InterpValue::List),
        BuiltinValue::Slice(values) => values
            .into_iter()
            .map(from_builtin)
            .collect::<Result<Vec<_>, _>>()
            .map(SliceValue::new)
            .map(InterpValue::Slice),
        BuiltinValue::Map(entries) => entries
            .into_iter()
            .map(|(key, value)| Ok((from_builtin(key)?, from_builtin(value)?)))
            .collect::<Result<Vec<_>, _>>()
            .map(MapValue::new)
            .map(InterpValue::Map),
        BuiltinValue::Record(fields) => fields
            .into_iter()
            .map(|(field, value)| Ok((field, from_builtin(value)?)))
            .collect::<Result<Vec<_>, _>>()
            .map(RecordValue::new)
            .map(InterpValue::Record),
        BuiltinValue::Range { start, end, bounds } => Ok(InterpValue::Range(RangeValue {
            start: Box::new(from_builtin(*start)?),
            end: Box::new(from_builtin(*end)?),
            bounds: from_builtin_range_bounds(bounds),
        })),
        BuiltinValue::OptionNone => Ok(InterpValue::OptionNone),
        BuiltinValue::OptionSome(value) => from_builtin(*value)
            .map(Box::new)
            .map(InterpValue::OptionSome),
        BuiltinValue::ResultOk(value) => from_builtin(*value).map(|value| InterpValue::Variant {
            name: "Ok".to_owned(),
            fields: vec![value],
        }),
        BuiltinValue::ResultErr(value) => from_builtin(*value).map(|value| InterpValue::Variant {
            name: "Err".to_owned(),
            fields: vec![value],
        }),
        BuiltinValue::Variant { name, fields } => fields
            .into_iter()
            .map(from_builtin)
            .collect::<Result<Vec<_>, _>>()
            .map(|fields| InterpValue::Variant { name, fields }),
        other => Err(AdapterError::UnsupportedValue(format!("{other:?}"))),
    }
}

fn into_builtin_range_bounds(bounds: RangeBounds) -> BuiltinRangeBounds {
    match bounds {
        RangeBounds::ClosedClosed => BuiltinRangeBounds::ClosedClosed,
        RangeBounds::ClosedOpen => BuiltinRangeBounds::ClosedOpen,
        RangeBounds::OpenOpen => BuiltinRangeBounds::OpenOpen,
        RangeBounds::OpenClosed => BuiltinRangeBounds::OpenClosed,
    }
}

fn from_builtin_range_bounds(bounds: BuiltinRangeBounds) -> RangeBounds {
    match bounds {
        BuiltinRangeBounds::ClosedClosed => RangeBounds::ClosedClosed,
        BuiltinRangeBounds::ClosedOpen => RangeBounds::ClosedOpen,
        BuiltinRangeBounds::OpenOpen => RangeBounds::OpenOpen,
        BuiltinRangeBounds::OpenClosed => RangeBounds::OpenClosed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_preserves_slice_values() {
        let value = InterpValue::Slice(SliceValue::new(vec![
            InterpValue::i32(1),
            InterpValue::i32(2),
        ]));

        assert_eq!(
            into_builtin(value).expect("slice should convert to builtin"),
            BuiltinValue::Slice(vec![BuiltinValue::I32(1), BuiltinValue::I32(2)])
        );

        assert_eq!(
            from_builtin(BuiltinValue::Slice(vec![
                BuiltinValue::I32(3),
                BuiltinValue::I32(4),
            ]))
            .expect("slice should convert from builtin"),
            InterpValue::Slice(SliceValue::new(vec![
                InterpValue::i32(3),
                InterpValue::i32(4)
            ]))
        );
    }

    #[test]
    fn adapter_preserves_range_bounds_and_endpoints() {
        let value = InterpValue::Range(RangeValue {
            start: Box::new(InterpValue::i32(1)),
            end: Box::new(InterpValue::i32(5)),
            bounds: RangeBounds::OpenClosed,
        });

        assert_eq!(
            into_builtin(value).expect("range should convert to builtin"),
            BuiltinValue::Range {
                start: Box::new(BuiltinValue::I32(1)),
                end: Box::new(BuiltinValue::I32(5)),
                bounds: BuiltinRangeBounds::OpenClosed,
            }
        );

        assert_eq!(
            from_builtin(BuiltinValue::Range {
                start: Box::new(BuiltinValue::I32(2)),
                end: Box::new(BuiltinValue::I32(8)),
                bounds: BuiltinRangeBounds::ClosedOpen,
            })
            .expect("range should convert from builtin"),
            InterpValue::Range(RangeValue {
                start: Box::new(InterpValue::i32(2)),
                end: Box::new(InterpValue::i32(8)),
                bounds: RangeBounds::ClosedOpen,
            })
        );
    }
}

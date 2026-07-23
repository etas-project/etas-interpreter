use etas_builtin::{BuiltinRangeBounds, BuiltinValue};
use etas_types::{PrimitiveType, TypeId};

use crate::value::{
    ArrayValue, InterpValue, ListValue, MapValue, NumericValue, RangeBounds, RangeValue,
    RecordValue, SetValue, SliceValue,
};

use super::{AbiShape, AdapterError, PureAbiProjector, SequenceKind};

pub(in crate::intrinsic::pure) fn from_builtin_for_type(
    value: BuiltinValue,
    ty: TypeId,
    projector: &PureAbiProjector,
) -> Result<InterpValue, AdapterError> {
    let shape = projector
        .shape(ty)
        .cloned()
        .ok_or(AdapterError::MissingType(ty))?;
    match shape {
        AbiShape::Nominal { representation } => Ok(InterpValue::Nominal {
            ty,
            value: Box::new(from_builtin_for_type(value, representation, projector)?),
        }),
        AbiShape::Primitive(primitive) => from_builtin_primitive(value, ty, primitive),
        AbiShape::Array(inner) => {
            sequence_from_builtin(value, ty, inner, projector, SequenceKind::Array)
        }
        AbiShape::List(inner) => {
            sequence_from_builtin(value, ty, inner, projector, SequenceKind::List)
        }
        AbiShape::Slice(inner) => {
            sequence_from_builtin(value, ty, inner, projector, SequenceKind::Slice)
        }
        AbiShape::Set(inner) => {
            sequence_from_builtin(value, ty, inner, projector, SequenceKind::Set)
        }
        AbiShape::Map {
            key,
            value: value_ty,
        } => {
            let BuiltinValue::Map(entries) = value else {
                return Err(builtin_type_mismatch(ty, &value));
            };
            entries
                .into_iter()
                .map(|(entry_key, entry_value)| {
                    Ok((
                        from_builtin_for_type(entry_key, key, projector)?,
                        from_builtin_for_type(entry_value, value_ty, projector)?,
                    ))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(MapValue::new)
                .map(InterpValue::Map)
        }
        AbiShape::Range(index) => {
            let BuiltinValue::Range { start, end, bounds } = value else {
                return Err(builtin_type_mismatch(ty, &value));
            };
            Ok(InterpValue::Range(RangeValue {
                start: Box::new(from_builtin_for_type(*start, index, projector)?),
                end: Box::new(from_builtin_for_type(*end, index, projector)?),
                bounds: from_builtin_range_bounds(bounds),
            }))
        }
        AbiShape::Option(inner) => match value {
            BuiltinValue::OptionNone => Ok(InterpValue::OptionNone),
            BuiltinValue::OptionSome(value) => from_builtin_for_type(*value, inner, projector)
                .map(Box::new)
                .map(InterpValue::OptionSome),
            other => Err(builtin_type_mismatch(ty, &other)),
        },
        AbiShape::Result { ok, err } => match value {
            BuiltinValue::ResultOk(value) => {
                from_builtin_for_type(*value, ok, projector).map(|value| InterpValue::Variant {
                    name: "Ok".to_owned(),
                    fields: vec![value],
                })
            }
            BuiltinValue::ResultErr(value) => {
                from_builtin_for_type(*value, err, projector).map(|value| InterpValue::Variant {
                    name: "Err".to_owned(),
                    fields: vec![value],
                })
            }
            other => Err(builtin_type_mismatch(ty, &other)),
        },
        AbiShape::Record(fields) => {
            let BuiltinValue::Record(values) = value else {
                return Err(builtin_type_mismatch(ty, &value));
            };
            checked_record_from_builtin(values, ty, &fields, projector)
        }
        AbiShape::Tuple(types) => {
            let BuiltinValue::Variant { name, fields } = value else {
                return Err(builtin_type_mismatch(ty, &value));
            };
            if name != "Tuple" || fields.len() != types.len() {
                return Err(AdapterError::TypeMismatch {
                    expected: ty,
                    actual: format!("builtin variant {name} with {} field(s)", fields.len()),
                });
            }
            fields
                .into_iter()
                .zip(types)
                .map(|(value, ty)| from_builtin_for_type(value, ty, projector))
                .collect::<Result<Vec<_>, _>>()
                .map(InterpValue::Tuple)
        }
        AbiShape::Enum => from_builtin_enum(value, ty),
        AbiShape::Refined { base } => from_builtin_for_type(value, base, projector),
        AbiShape::Trust { wrapper, inner } => Ok(InterpValue::Trust {
            wrapper,
            value: Box::new(from_builtin_for_type(value, inner, projector)?),
        }),
        AbiShape::Unsupported(description) => Err(AdapterError::UnsupportedValue(format!(
            "checked pure intrinsic ABI does not support result type {ty:?}: {description}"
        ))),
    }
}

fn sequence_from_builtin(
    value: BuiltinValue,
    ty: TypeId,
    inner: TypeId,
    projector: &PureAbiProjector,
    kind: SequenceKind,
) -> Result<InterpValue, AdapterError> {
    let values = match (kind, value) {
        (SequenceKind::Array, BuiltinValue::Array(values))
        | (SequenceKind::List, BuiltinValue::List(values))
        | (SequenceKind::Slice, BuiltinValue::Slice(values))
        | (SequenceKind::Set, BuiltinValue::Set(values)) => values,
        (_, other) => return Err(builtin_type_mismatch(ty, &other)),
    };
    let values = values
        .into_iter()
        .map(|value| from_builtin_for_type(value, inner, projector))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(match kind {
        SequenceKind::Array => InterpValue::Array(ArrayValue::new(values)),
        SequenceKind::List => InterpValue::List(ListValue::new(values)),
        SequenceKind::Slice => InterpValue::Slice(SliceValue::new(values)),
        SequenceKind::Set => InterpValue::Set(SetValue::new(values)),
    })
}

fn checked_record_from_builtin(
    mut values: Vec<(String, BuiltinValue)>,
    ty: TypeId,
    fields: &[etas_types::FieldType],
    projector: &PureAbiProjector,
) -> Result<InterpValue, AdapterError> {
    if values.len() != fields.len() {
        return Err(AdapterError::TypeMismatch {
            expected: ty,
            actual: format!("builtin record with {} field(s)", values.len()),
        });
    }
    fields
        .iter()
        .map(|field| {
            let Some(index) = values.iter().position(|(name, _)| name == &field.name) else {
                return Err(AdapterError::TypeMismatch {
                    expected: ty,
                    actual: format!("builtin record missing field `{}`", field.name),
                });
            };
            let (_, value) = values.remove(index);
            Ok((
                field.name.clone(),
                from_builtin_for_type(value, field.ty, projector)?,
            ))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(RecordValue::new)
        .map(InterpValue::Record)
}

fn from_builtin_primitive(
    value: BuiltinValue,
    ty: TypeId,
    primitive: PrimitiveType,
) -> Result<InterpValue, AdapterError> {
    match (primitive, value) {
        (PrimitiveType::Unit, BuiltinValue::Unit) => Ok(InterpValue::Unit),
        (PrimitiveType::Bool, BuiltinValue::Bool(value)) => Ok(InterpValue::Bool(value)),
        (PrimitiveType::String, BuiltinValue::String(value)) => Ok(InterpValue::String(value)),
        (PrimitiveType::Bytes, BuiltinValue::Bytes(value)) => Ok(InterpValue::Bytes(value)),
        (PrimitiveType::I8, BuiltinValue::I8(value)) => {
            Ok(InterpValue::Number(NumericValue::I8(value)))
        }
        (PrimitiveType::I16, BuiltinValue::I16(value)) => {
            Ok(InterpValue::Number(NumericValue::I16(value)))
        }
        (PrimitiveType::I32, BuiltinValue::I32(value)) => {
            Ok(InterpValue::Number(NumericValue::I32(value)))
        }
        (PrimitiveType::I64, BuiltinValue::I64(value)) => {
            Ok(InterpValue::Number(NumericValue::I64(value)))
        }
        (PrimitiveType::I128, BuiltinValue::I128(value)) => {
            Ok(InterpValue::Number(NumericValue::I128(value)))
        }
        (PrimitiveType::ISize, BuiltinValue::Isize(value)) => Ok(InterpValue::Number(
            NumericValue::ISize(i64::try_from(value).map_err(|_| {
                AdapterError::UnsupportedValue(format!("isize value {value} exceeds Etas ABI"))
            })?),
        )),
        (PrimitiveType::U8, BuiltinValue::U8(value)) => {
            Ok(InterpValue::Number(NumericValue::U8(value)))
        }
        (PrimitiveType::U16, BuiltinValue::U16(value)) => {
            Ok(InterpValue::Number(NumericValue::U16(value)))
        }
        (PrimitiveType::U32, BuiltinValue::U32(value)) => {
            Ok(InterpValue::Number(NumericValue::U32(value)))
        }
        (PrimitiveType::U64, BuiltinValue::U64(value)) => {
            Ok(InterpValue::Number(NumericValue::U64(value)))
        }
        (PrimitiveType::U128, BuiltinValue::U128(value)) => {
            Ok(InterpValue::Number(NumericValue::U128(value)))
        }
        (PrimitiveType::USize, BuiltinValue::Usize(value)) => Ok(InterpValue::Number(
            NumericValue::USize(u64::try_from(value).map_err(|_| {
                AdapterError::UnsupportedValue(format!("usize value {value} exceeds Etas ABI"))
            })?),
        )),
        (PrimitiveType::F32, BuiltinValue::F32(value)) => {
            Ok(InterpValue::Number(NumericValue::F32(value.to_bits())))
        }
        (PrimitiveType::F64, BuiltinValue::F64(value)) => {
            Ok(InterpValue::Number(NumericValue::F64(value.to_bits())))
        }
        (_, other) => Err(builtin_type_mismatch(ty, &other)),
    }
}

fn from_builtin_enum(value: BuiltinValue, ty: TypeId) -> Result<InterpValue, AdapterError> {
    let BuiltinValue::Variant { name, fields } = value else {
        return Err(builtin_type_mismatch(ty, &value));
    };
    fields
        .into_iter()
        .map(from_builtin)
        .collect::<Result<Vec<_>, _>>()
        .map(|fields| InterpValue::Variant { name, fields })
}

fn builtin_type_mismatch(expected: TypeId, actual: &BuiltinValue) -> AdapterError {
    AdapterError::TypeMismatch {
        expected,
        actual: format!("{actual:?}"),
    }
}

pub(in crate::intrinsic::pure) fn from_builtin(
    value: BuiltinValue,
) -> Result<InterpValue, AdapterError> {
    match value {
        BuiltinValue::Unit => Ok(InterpValue::Unit),
        BuiltinValue::Bool(value) => Ok(InterpValue::Bool(value)),
        BuiltinValue::I8(value) => Ok(InterpValue::Number(NumericValue::I8(value))),
        BuiltinValue::I16(value) => Ok(InterpValue::Number(NumericValue::I16(value))),
        BuiltinValue::I32(value) => Ok(InterpValue::i32(value)),
        BuiltinValue::I64(value) => Ok(InterpValue::i64(value)),
        BuiltinValue::I128(value) => Ok(InterpValue::Number(NumericValue::I128(value))),
        BuiltinValue::Isize(value) => Ok(InterpValue::Number(NumericValue::ISize(
            i64::try_from(value).map_err(|_| {
                AdapterError::UnsupportedValue(format!("isize value {value} exceeds Etas ABI"))
            })?,
        ))),
        BuiltinValue::U8(value) => Ok(InterpValue::Number(NumericValue::U8(value))),
        BuiltinValue::U16(value) => Ok(InterpValue::Number(NumericValue::U16(value))),
        BuiltinValue::U32(value) => Ok(InterpValue::Number(NumericValue::U32(value))),
        BuiltinValue::U64(value) => Ok(InterpValue::Number(NumericValue::U64(value))),
        BuiltinValue::U128(value) => Ok(InterpValue::Number(NumericValue::U128(value))),
        BuiltinValue::Usize(value) => Ok(InterpValue::Number(NumericValue::USize(
            u64::try_from(value).map_err(|_| {
                AdapterError::UnsupportedValue(format!("usize value {value} exceeds Etas ABI"))
            })?,
        ))),
        BuiltinValue::F32(value) => Ok(InterpValue::Number(NumericValue::F32(value.to_bits()))),
        BuiltinValue::F64(value) => Ok(InterpValue::Number(NumericValue::F64(value.to_bits()))),
        BuiltinValue::Char(value) => Err(AdapterError::UnsupportedValue(format!(
            "char builtin value `{value}` has no interpreter ABI representation"
        ))),
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
        BuiltinValue::Set(values) => values
            .into_iter()
            .map(from_builtin)
            .collect::<Result<Vec<_>, _>>()
            .map(SetValue::new)
            .map(InterpValue::Set),
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

use etas_builtin::{BuiltinRangeBounds, BuiltinValue};
use etas_types::{PrimitiveType, TypeId};

use crate::value::{InterpValue, NumericValue, RangeBounds};

use super::{AbiShape, AdapterError, PureAbiProjector, SequenceKind};

pub(in crate::intrinsic::pure) fn into_builtin_for_type(
    value: InterpValue,
    ty: TypeId,
    projector: &PureAbiProjector,
) -> Result<BuiltinValue, AdapterError> {
    let shape = projector
        .shape(ty)
        .cloned()
        .ok_or(AdapterError::MissingType(ty))?;
    match shape {
        AbiShape::Nominal { representation } => {
            let InterpValue::Nominal {
                ty: runtime_ty,
                value,
            } = value
            else {
                return Err(type_mismatch(ty, &value));
            };
            if runtime_ty != ty {
                return Err(AdapterError::NominalIdentity {
                    expected: ty,
                    actual: runtime_ty,
                });
            }
            into_builtin_for_type(*value, representation, projector)
        }
        AbiShape::Primitive(primitive) => into_builtin_primitive(value, ty, primitive),
        AbiShape::Array(inner) => {
            sequence_into_builtin(value, ty, inner, projector, SequenceKind::Array)
        }
        AbiShape::List(inner) => {
            sequence_into_builtin(value, ty, inner, projector, SequenceKind::List)
        }
        AbiShape::Slice(inner) => {
            sequence_into_builtin(value, ty, inner, projector, SequenceKind::Slice)
        }
        AbiShape::Set(inner) => {
            sequence_into_builtin(value, ty, inner, projector, SequenceKind::Set)
        }
        AbiShape::Map {
            key,
            value: value_ty,
        } => {
            let InterpValue::Map(entries) = value else {
                return Err(type_mismatch(ty, &value));
            };
            entries
                .snapshot()
                .into_iter()
                .map(|(entry_key, entry_value)| {
                    Ok((
                        into_builtin_for_type(entry_key, key, projector)?,
                        into_builtin_for_type(entry_value, value_ty, projector)?,
                    ))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(BuiltinValue::Map)
        }
        AbiShape::Range(index) => {
            let InterpValue::Range(range) = value else {
                return Err(type_mismatch(ty, &value));
            };
            Ok(BuiltinValue::Range {
                start: Box::new(into_builtin_for_type(*range.start, index, projector)?),
                end: Box::new(into_builtin_for_type(*range.end, index, projector)?),
                bounds: into_builtin_range_bounds(range.bounds),
            })
        }
        AbiShape::Option(inner) => match value {
            InterpValue::OptionNone => Ok(BuiltinValue::OptionNone),
            InterpValue::OptionSome(value) => into_builtin_for_type(*value, inner, projector)
                .map(Box::new)
                .map(BuiltinValue::OptionSome),
            other => Err(type_mismatch(ty, &other)),
        },
        AbiShape::Result { ok, err } => match value {
            InterpValue::Variant { name, mut fields } if name == "Ok" && fields.len() == 1 => {
                into_builtin_for_type(fields.remove(0), ok, projector)
                    .map(Box::new)
                    .map(BuiltinValue::ResultOk)
            }
            InterpValue::Variant { name, mut fields } if name == "Err" && fields.len() == 1 => {
                into_builtin_for_type(fields.remove(0), err, projector)
                    .map(Box::new)
                    .map(BuiltinValue::ResultErr)
            }
            other => Err(type_mismatch(ty, &other)),
        },
        AbiShape::Record(expected_fields) => {
            let InterpValue::Record(fields) = value else {
                return Err(type_mismatch(ty, &value));
            };
            checked_record_into_builtin(fields.snapshot(), ty, &expected_fields, projector)
        }
        AbiShape::Tuple(types) => {
            let InterpValue::Tuple(values) = value else {
                return Err(type_mismatch(ty, &value));
            };
            if values.len() != types.len() {
                return Err(type_mismatch(ty, &InterpValue::Tuple(values)));
            }
            values
                .into_iter()
                .zip(types)
                .map(|(value, ty)| into_builtin_for_type(value, ty, projector))
                .collect::<Result<Vec<_>, _>>()
                .map(|fields| BuiltinValue::Variant {
                    name: "Tuple".to_owned(),
                    fields,
                })
        }
        AbiShape::Enum => into_builtin_enum(value, ty),
        AbiShape::Refined { base } => into_builtin_for_type(value, base, projector),
        AbiShape::Trust { wrapper, inner } => {
            let InterpValue::Trust {
                wrapper: runtime_wrapper,
                value,
            } = value
            else {
                return Err(type_mismatch(ty, &value));
            };
            if runtime_wrapper != wrapper {
                return Err(type_mismatch(
                    ty,
                    &InterpValue::Trust {
                        wrapper: runtime_wrapper,
                        value,
                    },
                ));
            }
            into_builtin_for_type(*value, inner, projector)
        }
        AbiShape::Unsupported(description) => Err(AdapterError::UnsupportedValue(format!(
            "checked pure intrinsic ABI does not support type {ty:?}: {description}"
        ))),
    }
}

fn sequence_into_builtin(
    value: InterpValue,
    ty: TypeId,
    inner: TypeId,
    projector: &PureAbiProjector,
    kind: SequenceKind,
) -> Result<BuiltinValue, AdapterError> {
    let values = match (kind, value) {
        (SequenceKind::Array, InterpValue::Array(values)) => values.snapshot(),
        (SequenceKind::List, InterpValue::List(values)) => values.snapshot(),
        (SequenceKind::Slice, InterpValue::Slice(values)) => values.snapshot(),
        (SequenceKind::Set, InterpValue::Set(values)) => values.snapshot(),
        (_, other) => return Err(type_mismatch(ty, &other)),
    };
    let values = values
        .into_iter()
        .map(|value| into_builtin_for_type(value, inner, projector))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(match kind {
        SequenceKind::Array => BuiltinValue::Array(values),
        SequenceKind::List => BuiltinValue::List(values),
        SequenceKind::Slice => BuiltinValue::Slice(values),
        SequenceKind::Set => BuiltinValue::Set(values),
    })
}

fn checked_record_into_builtin(
    mut values: Vec<(String, InterpValue)>,
    ty: TypeId,
    fields: &[etas_types::FieldType],
    projector: &PureAbiProjector,
) -> Result<BuiltinValue, AdapterError> {
    if values.len() != fields.len() {
        return Err(AdapterError::TypeMismatch {
            expected: ty,
            actual: format!("record with {} field(s)", values.len()),
        });
    }
    fields
        .iter()
        .map(|field| {
            let Some(index) = values.iter().position(|(name, _)| name == &field.name) else {
                return Err(AdapterError::TypeMismatch {
                    expected: ty,
                    actual: format!("record missing field `{}`", field.name),
                });
            };
            let (_, value) = values.remove(index);
            Ok((
                field.name.clone(),
                into_builtin_for_type(value, field.ty, projector)?,
            ))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(BuiltinValue::Record)
}

fn into_builtin_primitive(
    value: InterpValue,
    ty: TypeId,
    primitive: PrimitiveType,
) -> Result<BuiltinValue, AdapterError> {
    match (primitive, value) {
        (PrimitiveType::Unit, InterpValue::Unit) => Ok(BuiltinValue::Unit),
        (PrimitiveType::Bool, InterpValue::Bool(value)) => Ok(BuiltinValue::Bool(value)),
        (PrimitiveType::String, InterpValue::String(value)) => Ok(BuiltinValue::String(value)),
        (PrimitiveType::Bytes, InterpValue::Bytes(value)) => Ok(BuiltinValue::Bytes(value)),
        (expected, InterpValue::Number(value)) if value.primitive() == expected => {
            numeric_into_builtin(value)
        }
        (_, other) => Err(type_mismatch(ty, &other)),
    }
}

fn numeric_into_builtin(value: NumericValue) -> Result<BuiltinValue, AdapterError> {
    Ok(match value {
        NumericValue::I8(value) => BuiltinValue::I8(value),
        NumericValue::I16(value) => BuiltinValue::I16(value),
        NumericValue::I32(value) => BuiltinValue::I32(value),
        NumericValue::I64(value) => BuiltinValue::I64(value),
        NumericValue::I128(value) => BuiltinValue::I128(value),
        NumericValue::ISize(value) => {
            BuiltinValue::Isize(isize::try_from(value).map_err(|_| {
                AdapterError::UnsupportedValue(format!("isize value {value} exceeds host ABI"))
            })?)
        }
        NumericValue::U8(value) => BuiltinValue::U8(value),
        NumericValue::U16(value) => BuiltinValue::U16(value),
        NumericValue::U32(value) => BuiltinValue::U32(value),
        NumericValue::U64(value) => BuiltinValue::U64(value),
        NumericValue::U128(value) => BuiltinValue::U128(value),
        NumericValue::USize(value) => {
            BuiltinValue::Usize(usize::try_from(value).map_err(|_| {
                AdapterError::UnsupportedValue(format!("usize value {value} exceeds host ABI"))
            })?)
        }
        NumericValue::F32(bits) => BuiltinValue::F32(f32::from_bits(bits)),
        NumericValue::F64(bits) => BuiltinValue::F64(f64::from_bits(bits)),
    })
}

fn into_builtin_enum(value: InterpValue, ty: TypeId) -> Result<BuiltinValue, AdapterError> {
    let InterpValue::Variant { name, fields } = value else {
        return Err(type_mismatch(ty, &value));
    };
    fields
        .into_iter()
        .map(into_builtin)
        .collect::<Result<Vec<_>, _>>()
        .map(|fields| BuiltinValue::Variant { name, fields })
}

pub(in crate::intrinsic::pure) fn type_mismatch(
    expected: TypeId,
    actual: &InterpValue,
) -> AdapterError {
    AdapterError::TypeMismatch {
        expected,
        actual: format!("{actual:?}"),
    }
}

pub(in crate::intrinsic::pure) fn into_builtin(
    value: InterpValue,
) -> Result<BuiltinValue, AdapterError> {
    match value {
        InterpValue::Unit => Ok(BuiltinValue::Unit),
        InterpValue::Bool(value) => Ok(BuiltinValue::Bool(value)),
        InterpValue::Number(value) => numeric_into_builtin(value),
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
        InterpValue::Set(values) => values
            .snapshot()
            .into_iter()
            .map(into_builtin)
            .collect::<Result<Vec<_>, _>>()
            .map(BuiltinValue::Set),
        InterpValue::Record(fields) => fields
            .snapshot()
            .into_iter()
            .map(|(field, value)| Ok((field, into_builtin(value)?)))
            .collect::<Result<Vec<_>, _>>()
            .map(BuiltinValue::Record),
        InterpValue::Range(range) => Ok(BuiltinValue::Range {
            start: Box::new(into_builtin(*range.start)?),
            end: Box::new(into_builtin(*range.end)?),
            bounds: into_builtin_range_bounds(range.bounds),
        }),
        InterpValue::OptionNone => Ok(BuiltinValue::OptionNone),
        InterpValue::OptionSome(value) => into_builtin(*value)
            .map(Box::new)
            .map(BuiltinValue::OptionSome),
        InterpValue::Variant { name, mut fields } if name == "Ok" && fields.len() == 1 => {
            into_builtin(fields.remove(0))
                .map(Box::new)
                .map(BuiltinValue::ResultOk)
        }
        InterpValue::Variant { name, mut fields } if name == "Err" && fields.len() == 1 => {
            into_builtin(fields.remove(0))
                .map(Box::new)
                .map(BuiltinValue::ResultErr)
        }
        InterpValue::Variant { name, fields } => fields
            .into_iter()
            .map(into_builtin)
            .collect::<Result<Vec<_>, _>>()
            .map(|fields| BuiltinValue::Variant { name, fields }),
        InterpValue::Nominal { .. } => Err(AdapterError::UnsupportedValue(
            "nominal values require a checked ABI projection".to_owned(),
        )),
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

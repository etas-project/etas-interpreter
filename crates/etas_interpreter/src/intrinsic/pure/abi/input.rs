use etas_builtin::{BuiltinRangeBounds, BuiltinValue};
use etas_types::{PrimitiveType, TypeId};

use crate::value::{InterpValue, NumericValue, RangeBounds, RecordValue};

use super::{AbiShape, AdapterError, PureAbiProjector, SequenceKind};

#[cfg(test)]
mod tests;

pub(in crate::intrinsic::pure) fn into_builtin_for_type(
    mut value: InterpValue,
    mut ty: TypeId,
    projector: &PureAbiProjector,
) -> Result<BuiltinValue, AdapterError> {
    let mut remaining_steps = None;
    loop {
        let shape = projector.shape(ty).ok_or(AdapterError::MissingType(ty))?;
        if matches!(
            shape,
            AbiShape::Nominal { .. } | AbiShape::Refined { .. } | AbiShape::Trust { .. }
        ) {
            let remaining = match remaining_steps {
                Some(remaining) => remaining,
                None => projector.wrapper_steps(ty).ok_or_else(|| {
                    AdapterError::UnsupportedValue(
                        "checked input ABI is missing its prepared wrapper walk".into(),
                    )
                })?,
            };
            if remaining == 0 {
                return Err(AdapterError::UnsupportedValue(
                    "cyclic checked input ABI".into(),
                ));
            }
            remaining_steps = Some(remaining - 1);
        }
        return match shape {
            AbiShape::Nominal { representation } => {
                let InterpValue::Nominal {
                    ty: runtime_ty,
                    value: inner,
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
                value = inner.into_value();
                ty = *representation;
                continue;
            }
            AbiShape::Primitive(primitive) => into_builtin_primitive(value, ty, *primitive),
            AbiShape::Array(inner) => {
                sequence_into_builtin(value, ty, *inner, projector, SequenceKind::Array)
            }
            AbiShape::List(inner) => {
                sequence_into_builtin(value, ty, *inner, projector, SequenceKind::List)
            }
            AbiShape::Slice(inner) => {
                sequence_into_builtin(value, ty, *inner, projector, SequenceKind::Slice)
            }
            AbiShape::Set(inner) => {
                sequence_into_builtin(value, ty, *inner, projector, SequenceKind::Set)
            }
            AbiShape::Map {
                key,
                value: value_ty,
            } => {
                let InterpValue::Map(entries) = value else {
                    return Err(type_mismatch(ty, &value));
                };
                entries
                    .try_map_values(|(entry_key, entry_value)| {
                        Ok((
                            into_builtin_for_type(entry_key, *key, projector)?,
                            into_builtin_for_type(entry_value, *value_ty, projector)?,
                        ))
                    })
                    .map(BuiltinValue::Map)
            }
            AbiShape::Range(index) => {
                let InterpValue::Range(range) = value else {
                    return Err(type_mismatch(ty, &value));
                };
                Ok(BuiltinValue::Range {
                    start: Box::new(into_builtin_for_type(*range.start, *index, projector)?),
                    end: Box::new(into_builtin_for_type(*range.end, *index, projector)?),
                    bounds: into_builtin_range_bounds(range.bounds),
                })
            }
            AbiShape::Option(inner) => match value {
                InterpValue::OptionNone => Ok(BuiltinValue::OptionNone),
                InterpValue::OptionSome(value) => {
                    into_builtin_for_type(value.into_value(), *inner, projector)
                        .map(Box::new)
                        .map(BuiltinValue::OptionSome)
                }
                other => Err(type_mismatch(ty, &other)),
            },
            AbiShape::Result { ok, err } => match value {
                InterpValue::Variant { name, fields } if name == "Ok" && fields.len() == 1 => {
                    into_builtin_for_type(
                        fields.into_single().ok_or(AdapterError::Arity {
                            expected: 1,
                            actual: 0,
                        })?,
                        *ok,
                        projector,
                    )
                    .map(Box::new)
                    .map(BuiltinValue::ResultOk)
                }
                InterpValue::Variant { name, fields } if name == "Err" && fields.len() == 1 => {
                    into_builtin_for_type(
                        fields.into_single().ok_or(AdapterError::Arity {
                            expected: 1,
                            actual: 0,
                        })?,
                        *err,
                        projector,
                    )
                    .map(Box::new)
                    .map(BuiltinValue::ResultErr)
                }
                other => Err(type_mismatch(ty, &other)),
            },
            AbiShape::Record(expected_fields) => {
                let InterpValue::Record(fields) = value else {
                    return Err(type_mismatch(ty, &value));
                };
                checked_record_into_builtin(fields, ty, expected_fields, projector)
            }
            AbiShape::Tuple(types) => {
                let InterpValue::Tuple(values) = value else {
                    return Err(type_mismatch(ty, &value));
                };
                if values.len() != types.len() {
                    return Err(type_mismatch(ty, &InterpValue::Tuple(values)));
                }
                values
                    .try_map_indexed(|index, value| {
                        into_builtin_for_type(value, types[index], projector)
                    })
                    .map(|fields| BuiltinValue::Variant {
                        name: "Tuple".to_owned(),
                        fields,
                    })
            }
            AbiShape::Enum(layout) => into_builtin_enum(value, ty, layout, projector),
            AbiShape::Refined { base } => {
                ty = *base;
                continue;
            }
            AbiShape::Trust { wrapper, inner } => {
                let InterpValue::Trust {
                    wrapper: runtime_wrapper,
                    value: inner_value,
                } = value
                else {
                    return Err(type_mismatch(ty, &value));
                };
                if runtime_wrapper != *wrapper {
                    return Err(type_mismatch(
                        ty,
                        &InterpValue::Trust {
                            wrapper: runtime_wrapper,
                            value: inner_value,
                        },
                    ));
                }
                value = inner_value.into_value();
                ty = *inner;
                continue;
            }
            AbiShape::Unsupported(description) => Err(AdapterError::UnsupportedValue(format!(
                "checked pure intrinsic ABI does not support type {ty:?}: {description}"
            ))),
        };
    }
}

fn sequence_into_builtin(
    value: InterpValue,
    ty: TypeId,
    inner: TypeId,
    projector: &PureAbiProjector,
    kind: SequenceKind,
) -> Result<BuiltinValue, AdapterError> {
    let project = |value| into_builtin_for_type(value, inner, projector);
    let values = match (kind, value) {
        (SequenceKind::Array, InterpValue::Array(values)) => values.try_map_values(project)?,
        (SequenceKind::List, InterpValue::List(values)) => values.try_map_values(project)?,
        (SequenceKind::Slice, InterpValue::Slice(values)) => values.try_map_values(project)?,
        (SequenceKind::Set, InterpValue::Set(values)) => values.try_map_values(project)?,
        (_, other) => return Err(type_mismatch(ty, &other)),
    };
    Ok(match kind {
        SequenceKind::Array => BuiltinValue::Array(values),
        SequenceKind::List => BuiltinValue::List(values),
        SequenceKind::Slice => BuiltinValue::Slice(values),
        SequenceKind::Set => BuiltinValue::Set(values),
    })
}

fn checked_record_into_builtin(
    values: RecordValue,
    ty: TypeId,
    layout: &super::record::RecordAbiLayout,
    projector: &PureAbiProjector,
) -> Result<BuiltinValue, AdapterError> {
    layout.check_arity(values.borrow().len(), ty)?;
    let projected = values.try_map_values(|(name, value)| {
        let field_type = layout.field_type(&name, ty)?;
        Ok((name, into_builtin_for_type(value, field_type, projector)?))
    })?;
    layout.reorder(projected, ty).map(BuiltinValue::Record)
}

fn into_builtin_primitive(
    value: InterpValue,
    ty: TypeId,
    primitive: PrimitiveType,
) -> Result<BuiltinValue, AdapterError> {
    match (primitive, value) {
        (PrimitiveType::Unit, InterpValue::Unit) => Ok(BuiltinValue::Unit),
        (PrimitiveType::Bool, InterpValue::Bool(value)) => Ok(BuiltinValue::Bool(value)),
        (PrimitiveType::String, InterpValue::String(value)) => {
            Ok(BuiltinValue::String(value.into_string()))
        }
        (PrimitiveType::Bytes, InterpValue::Bytes(value)) => {
            Ok(BuiltinValue::Bytes(value.into_vec()))
        }
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

fn into_builtin_enum(
    value: InterpValue,
    ty: TypeId,
    layout: &super::enum_layout::EnumAbiLayout,
    projector: &PureAbiProjector,
) -> Result<BuiltinValue, AdapterError> {
    let InterpValue::Nominal { ty: actual, value } = value else {
        return Err(type_mismatch(ty, &value));
    };
    if actual != ty {
        return Err(AdapterError::NominalIdentity {
            expected: ty,
            actual,
        });
    }
    let value = value.into_value();
    let InterpValue::Variant { name, fields } = value else {
        return Err(type_mismatch(ty, &value));
    };
    let types = layout.payload(&name, fields.len())?;
    fields
        .try_map_indexed(|index, value| into_builtin_for_type(value, types[index], projector))
        .map(|fields| BuiltinValue::Variant {
            name: name.into_string(),
            fields,
        })
}

pub(in crate::intrinsic::pure) fn type_mismatch(
    expected: TypeId,
    actual: &InterpValue,
) -> AdapterError {
    AdapterError::TypeMismatch {
        expected,
        actual: actual.kind_name().to_owned(),
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

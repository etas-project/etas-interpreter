use etas_types::{PrimitiveType, TypeId};

use crate::value::{InterpValue, StringValue};

use super::{AbiShape, AdapterError, PureAbiProjector, input::type_mismatch};

pub(in crate::intrinsic::pure) fn string_for_type<'a>(
    value: &'a InterpValue,
    ty: TypeId,
    projector: &PureAbiProjector,
) -> Result<&'a str, AdapterError> {
    shared_string_for_type(value, ty, projector).map(|text| text.as_str())
}

pub(in crate::intrinsic::pure) fn shared_string_for_type<'a>(
    value: &'a InterpValue,
    ty: TypeId,
    projector: &PureAbiProjector,
) -> Result<&'a StringValue, AdapterError> {
    let (value, ty) = primitive_for_type(value, ty, projector, PrimitiveType::String)?;
    match value {
        InterpValue::String(text) => Ok(text),
        other => Err(type_mismatch(ty, other)),
    }
}

pub(in crate::intrinsic::pure) fn bytes_for_type<'a>(
    value: &'a InterpValue,
    ty: TypeId,
    projector: &PureAbiProjector,
) -> Result<&'a [u8], AdapterError> {
    let (value, ty) = primitive_for_type(value, ty, projector, PrimitiveType::Bytes)?;
    match value {
        InterpValue::Bytes(bytes) => Ok(bytes),
        other => Err(type_mismatch(ty, other)),
    }
}

fn primitive_for_type<'a>(
    value: &'a InterpValue,
    ty: TypeId,
    projector: &PureAbiProjector,
    expected: PrimitiveType,
) -> Result<(&'a InterpValue, TypeId), AdapterError> {
    let (value, ty) = representation_for_type(value, ty, projector)?;
    if matches!(projector.shape(ty), Some(AbiShape::Primitive(primitive)) if *primitive == expected)
    {
        Ok((value, ty))
    } else {
        Err(type_mismatch(ty, value))
    }
}

pub(in crate::intrinsic::pure) fn representation_for_type<'a>(
    mut value: &'a InterpValue,
    mut ty: TypeId,
    projector: &PureAbiProjector,
) -> Result<(&'a InterpValue, TypeId), AdapterError> {
    let mut visited = std::collections::HashSet::new();
    loop {
        let shape = projector.shape(ty).ok_or(AdapterError::MissingType(ty))?;
        if matches!(
            shape,
            AbiShape::Nominal { .. } | AbiShape::Refined { .. } | AbiShape::Trust { .. }
        ) && !visited.insert(ty)
        {
            return Err(AdapterError::UnsupportedValue(
                "cyclic checked borrowed ABI".into(),
            ));
        }
        match shape {
            AbiShape::Nominal { representation } => {
                let InterpValue::Nominal {
                    ty: actual,
                    value: inner,
                } = value
                else {
                    return Err(type_mismatch(ty, value));
                };
                if *actual != ty {
                    return Err(AdapterError::NominalIdentity {
                        expected: ty,
                        actual: *actual,
                    });
                }
                ty = *representation;
                value = inner;
            }
            AbiShape::Refined { base } => ty = *base,
            AbiShape::Trust { wrapper, inner } => {
                let InterpValue::Trust {
                    wrapper: actual,
                    value: payload,
                } = value
                else {
                    return Err(type_mismatch(ty, value));
                };
                if wrapper != actual {
                    return Err(type_mismatch(ty, value));
                }
                ty = *inner;
                value = payload;
            }
            _ => return Ok((value, ty)),
        }
    }
}

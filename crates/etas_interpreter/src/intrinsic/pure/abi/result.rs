use etas_types::{TrustWrapper, TypeId};

use crate::value::InterpValue;

use super::{AbiShape, AdapterError, PureAbiProjector};

#[cfg(test)]
mod tests;

enum Wrapper {
    Nominal(TypeId),
    Trust(TrustWrapper),
}

pub(super) fn restore_wrappers<T>(
    value: T,
    mut ty: TypeId,
    projector: &PureAbiProjector,
    leaf: impl FnOnce(T, TypeId, &AbiShape) -> Result<InterpValue, AdapterError>,
) -> Result<InterpValue, AdapterError> {
    let mut wrappers = Vec::new();
    let mut remaining_steps = None;
    let shape = loop {
        let shape = projector.shape(ty).ok_or(AdapterError::MissingType(ty))?;
        if matches!(
            shape,
            AbiShape::Nominal { .. } | AbiShape::Trust { .. } | AbiShape::Refined { .. }
        ) {
            let remaining = match remaining_steps {
                Some(remaining) => remaining,
                None => projector.wrapper_steps(ty).ok_or_else(|| {
                    AdapterError::UnsupportedValue(
                        "checked result ABI is missing its prepared wrapper walk".into(),
                    )
                })?,
            };
            if remaining == 0 {
                return Err(AdapterError::UnsupportedValue(
                    "cyclic checked result ABI".into(),
                ));
            }
            remaining_steps = Some(remaining - 1);
        }
        match shape {
            AbiShape::Nominal { representation } => {
                wrappers.push(Wrapper::Nominal(ty));
                ty = *representation;
            }
            AbiShape::Trust { wrapper, inner } => {
                wrappers.push(Wrapper::Trust(*wrapper));
                ty = *inner;
            }
            AbiShape::Refined { base } => ty = *base,
            _ => break shape,
        }
    };
    let mut result = leaf(value, ty, shape)?;
    for wrapper in wrappers.into_iter().rev() {
        result = match wrapper {
            Wrapper::Nominal(ty) => InterpValue::Nominal {
                ty,
                value: crate::value::SharedValue::new(result),
            },
            Wrapper::Trust(wrapper) => InterpValue::Trust {
                wrapper,
                value: crate::value::SharedValue::new(result),
            },
        };
    }
    Ok(result)
}

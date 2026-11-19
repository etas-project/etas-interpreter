use etas_code::descriptor::{ParameterArgumentInfo, ReturnObservationInfo};
use etas_types::TypeId;

use crate::{
    eval::code::project::{Projector, optional},
    value::{CodeMetadataPayload as M, InterpValue as V},
};

pub(super) fn arguments(
    p: &Projector<'_>,
    ty: TypeId,
    arguments: Option<&[ParameterArgumentInfo]>,
) -> Result<V, String> {
    let Some(arguments) = arguments else {
        return Ok(optional(None));
    };
    let element = p.array_element(p.option(ty)?)?;
    let values = arguments
        .iter()
        .map(|argument| match argument {
            ParameterArgumentInfo::Bool(value) => p.variant(element, "Bool", vec![V::Bool(*value)]),
            ParameterArgumentInfo::Integer {
                negative,
                magnitude,
            } => p.named_variant(
                element,
                "Integer",
                vec![
                    ("negative", V::Bool(*negative)),
                    ("magnitude", V::String(magnitude.to_string().into())),
                ],
            ),
            ParameterArgumentInfo::Input { position } => p.named_variant(
                element,
                "Input",
                vec![("position", V::usize(*position as usize))],
            ),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(optional(Some(V::Array(values.into()))))
}

pub(super) fn observation(
    p: &Projector<'_>,
    ty: TypeId,
    value: &ReturnObservationInfo,
) -> Result<V, String> {
    let (name, values) = match value {
        ReturnObservationInfo::Any => ("Any", Vec::new()),
        ReturnObservationInfo::Unit => ("Unit", Vec::new()),
        ReturnObservationInfo::Bool(value) => ("Bool", vec![V::Bool(*value)]),
        ReturnObservationInfo::Result(value) => ("Result", vec![V::Bool(*value)]),
        ReturnObservationInfo::Option(value) => ("Option", vec![V::Bool(*value)]),
        ReturnObservationInfo::Variant { owner, name } => {
            return p.named_variant(
                ty,
                "Variant",
                vec![
                    (
                        "owner",
                        p.metadata(
                            p.variant_field(ty, "Variant", "owner")?,
                            M::Definition(owner.clone()),
                        )?,
                    ),
                    ("name", V::String(name.clone().into())),
                ],
            );
        }
    };
    p.variant(ty, name, values)
}

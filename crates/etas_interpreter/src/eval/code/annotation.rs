use etas_code::descriptor::AnnotationInfo;
use etas_types::TypeId;

use super::project::{Projector, optional};
use crate::value::{CodeMetadataPayload as M, InterpValue as V};

pub(super) fn annotations(
    p: &Projector<'_>,
    ty: TypeId,
    annotations: &[AnnotationInfo],
) -> Result<V, String> {
    let annotation_ty = p.array_element(ty)?;
    let argument_ty = p.array_element(p.field(annotation_ty, "arguments")?)?;
    let value_ty = p.field(argument_ty, "value")?;
    annotations
        .iter()
        .map(|annotation| {
            let arguments = annotation
                .arguments
                .iter()
                .map(|arg| {
                    p.record(
                        argument_ty,
                        vec![
                            (
                                "name",
                                optional(
                                    arg.name.as_ref().map(|name| V::String(name.clone().into())),
                                ),
                            ),
                            (
                                "value",
                                p.metadata(value_ty, M::StaticValue(arg.value.clone()))?,
                            ),
                        ],
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            p.record(
                annotation_ty,
                vec![
                    (
                        "path",
                        V::Array(
                            annotation
                                .path
                                .iter()
                                .map(|segment| V::String(segment.clone().into()))
                                .collect::<Vec<_>>()
                                .into(),
                        ),
                    ),
                    ("arguments", V::Array(arguments.into())),
                ],
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|values| V::Array(values.into()))
}

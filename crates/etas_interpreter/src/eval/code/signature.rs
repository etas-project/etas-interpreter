use etas_code::descriptor::{DescriptorGraph, GenericParameterKind, SignatureInfo};
use etas_types::TypeId;

use super::project::{Projector, optional};
use crate::value::{CodeMetadataPayload as M, InterpValue as V};

pub(super) fn signature(
    p: &Projector<'_>,
    ty: TypeId,
    graph: &DescriptorGraph,
    signature: &SignatureInfo,
) -> Result<V, String> {
    let parameter_ty = p.array_element(p.field(ty, "parameters")?)?;
    let parameters = signature
        .parameters
        .iter()
        .map(|parameter| {
            p.record(
                parameter_ty,
                vec![
                    (
                        "name",
                        optional(
                            parameter
                                .name
                                .as_ref()
                                .map(|name| V::String(name.clone().into())),
                        ),
                    ),
                    (
                        "ty",
                        p.metadata(
                            p.field(parameter_ty, "ty")?,
                            M::Type {
                                graph: graph.clone(),
                                root: parameter.ty,
                            },
                        )?,
                    ),
                ],
            )
        })
        .collect::<Result<Vec<_>, String>>()?;
    let generic_ty = p.array_element(p.field(ty, "generics")?)?;
    let spec_ty = p.array_element(p.field(generic_ty, "bounds")?)?;
    let generics =
        signature
            .generics
            .iter()
            .map(|parameter| {
                let binder = graph
                    .binder(*parameter)
                    .ok_or("signature has an invalid generic binder")?;
                let bounds = binder
                    .bounds
                    .iter()
                    .map(|bound| super::spec::named(p, spec_ty, graph, bound))
                    .collect::<Result<Vec<_>, String>>()?;
                p.record(
                    generic_ty,
                    vec![
                        (
                            "owner",
                            p.metadata(
                                p.field(generic_ty, "owner")?,
                                M::GenericOwner(binder.owner.clone()),
                            )?,
                        ),
                        (
                            "position",
                            V::usize(usize::try_from(binder.position).map_err(
                                |_| "generic parameter position exceeds the runtime ABI",
                            )?),
                        ),
                        ("name", V::String(binder.name.clone().into())),
                        (
                            "kind",
                            V::String(
                                match binder.kind {
                                    GenericParameterKind::Type => "type",
                                    GenericParameterKind::EffectRow => "effect",
                                }
                                .into(),
                            ),
                        ),
                        ("bounds", V::Array(bounds.into())),
                    ],
                )
            })
            .collect::<Result<Vec<_>, String>>()?;
    p.record(
        ty,
        vec![
            ("generics", V::Array(generics.into())),
            ("parameters", V::Array(parameters.into())),
            (
                "output",
                p.metadata(
                    p.field(ty, "output")?,
                    M::Type {
                        graph: graph.clone(),
                        root: signature.output,
                    },
                )?,
            ),
            (
                "declared_output",
                p.optional_metadata(
                    p.field(ty, "declared_output")?,
                    signature.declared_output.map(|root| M::Type {
                        graph: graph.clone(),
                        root,
                    }),
                )?,
            ),
            (
                "effects",
                p.optional_metadata(
                    p.field(ty, "effects")?,
                    signature.effects.map(|root| M::EffectRow {
                        graph: graph.clone(),
                        root,
                    }),
                )?,
            ),
        ],
    )
}

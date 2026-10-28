use etas_code::descriptor::{AttachedSpecInfo, DescriptorGraph, SpecInfo};
use etas_types::TypeId;

use super::project::{Projector, optional};
use crate::value::{CodeMetadataPayload as M, InterpValue as V};

pub(super) fn attached(
    p: &Projector<'_>,
    ty: TypeId,
    graph: &DescriptorGraph,
    specs: &[AttachedSpecInfo],
) -> Result<V, String> {
    let element = p.array_element(ty)?;
    specs
        .iter()
        .map(|spec| match spec {
            AttachedSpecInfo::Named(spec) => named(p, element, graph, spec),
            AttachedSpecInfo::Protocol {
                definition,
                arguments,
            } => reference(p, element, graph, definition, arguments),
            AttachedSpecInfo::InlineTrace(constraint) => p.record(
                element,
                vec![
                    ("definition", optional(None)),
                    ("arguments", V::Array(Vec::new().into())),
                    (
                        "trace",
                        p.optional_metadata(
                            p.field(element, "trace")?,
                            Some(M::TraceConstraint {
                                graph: graph.clone(),
                                value: constraint.clone(),
                            }),
                        )?,
                    ),
                ],
            ),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|specs| V::Array(specs.into()))
}

pub(super) fn named(
    p: &Projector<'_>,
    ty: TypeId,
    graph: &DescriptorGraph,
    spec: &SpecInfo,
) -> Result<V, String> {
    reference(p, ty, graph, &spec.definition, &spec.arguments)
}

fn reference(
    p: &Projector<'_>,
    ty: TypeId,
    graph: &DescriptorGraph,
    definition: &etas_code::DefinitionKey,
    arguments: &[etas_code::descriptor::TypeInfoRef],
) -> Result<V, String> {
    let argument = p.array_element(p.field(ty, "arguments")?)?;
    let arguments = arguments
        .iter()
        .map(|root| {
            p.metadata(
                argument,
                M::Type {
                    graph: graph.clone(),
                    root: *root,
                },
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    p.record(
        ty,
        vec![
            (
                "definition",
                p.optional_metadata(
                    p.field(ty, "definition")?,
                    Some(M::Definition(definition.clone())),
                )?,
            ),
            ("arguments", V::Array(arguments.into())),
            ("trace", optional(None)),
        ],
    )
}

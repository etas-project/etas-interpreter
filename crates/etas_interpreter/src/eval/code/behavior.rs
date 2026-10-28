use etas_code::descriptor::{DescriptorGraph, EffectRowInfoRef, EffectSummaryInfo};
use etas_types::TypeId;

use super::project::Projector;
use crate::value::{CodeMetadataPayload as M, InterpValue as V};

pub(super) fn effects(
    p: &Projector<'_>,
    ty: TypeId,
    declared: Option<(&DescriptorGraph, EffectRowInfoRef)>,
    current: Option<(&DescriptorGraph, &EffectSummaryInfo)>,
) -> Result<V, String> {
    p.record(
        ty,
        vec![
            (
                "declared_bound",
                p.optional_metadata(
                    p.field(ty, "declared_bound")?,
                    declared.map(|(graph, root)| M::EffectRow {
                        graph: graph.clone(),
                        root,
                    }),
                )?,
            ),
            (
                "escaping",
                p.optional_metadata(
                    p.field(ty, "escaping")?,
                    current.and_then(|(graph, summary)| {
                        summary.escaping.map(|root| M::EffectRow {
                            graph: graph.clone(),
                            root,
                        })
                    }),
                )?,
            ),
            (
                "requested",
                p.optional_metadata(
                    p.field(ty, "requested")?,
                    current.and_then(|(graph, summary)| {
                        summary.requested.map(|root| M::Actions {
                            graph: graph.clone(),
                            root,
                        })
                    }),
                )?,
            ),
            (
                "default_handled",
                p.optional_metadata(
                    p.field(ty, "default_handled")?,
                    current.and_then(|(graph, summary)| {
                        summary.default_handled.map(|root| M::Actions {
                            graph: graph.clone(),
                            root,
                        })
                    }),
                )?,
            ),
        ],
    )
}

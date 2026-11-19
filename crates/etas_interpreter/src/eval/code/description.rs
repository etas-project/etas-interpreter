use std::collections::BTreeSet;

use etas_code::descriptor::{DescriptorGraph, TypeInfoRef};

use super::project::Projector;

mod annotations;
mod effects;
mod observations;
mod trace;
mod types;
mod value_projection;
mod witness;

/// Encode compiler descriptions as ordinary values of the selected source ABI.
/// Named references stop recursive declaration expansion along the current path.
pub(super) struct Description<'p, 'a, 'g> {
    p: &'p Projector<'a>,
    graph: &'g DescriptorGraph,
    expanding: BTreeSet<TypeInfoRef>,
}

impl<'p, 'a, 'g> Description<'p, 'a, 'g> {
    pub fn new(p: &'p Projector<'a>, graph: &'g DescriptorGraph) -> Self {
        Self {
            p,
            graph,
            expanding: BTreeSet::new(),
        }
    }
}

pub(super) use annotations::annotation_value;
pub(super) use witness::witness;

pub(super) fn detail(
    p: &Projector<'_>,
    ty: etas_types::TypeId,
    detail: &etas_code::diagnostic::DescriptorDetail,
) -> Result<crate::value::InterpValue, String> {
    use etas_code::diagnostic::DescriptorDetail;
    let (variant, value) = match detail {
        DescriptorDetail::Type { graph, ty: root } => (
            "Type",
            Description::new(p, graph).type_info(p.variant_payload(ty, "Type")?, *root)?,
        ),
        DescriptorDetail::EffectRow { graph, row } => (
            "EffectRow",
            Description::new(p, graph).row(p.variant_payload(ty, "EffectRow")?, *row, false)?,
        ),
        DescriptorDetail::Spec { graph, spec } => (
            "Spec",
            Description::new(p, graph).spec(p.variant_payload(ty, "Spec")?, spec)?,
        ),
        DescriptorDetail::Trace { graph, trace } => (
            "Trace",
            Description::new(p, graph).trace_summary(p.variant_payload(ty, "Trace")?, *trace)?,
        ),
    };
    p.variant(ty, variant, vec![value])
}

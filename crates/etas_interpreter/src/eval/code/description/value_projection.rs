use etas_code::descriptor::ValueProjectionStep;
use etas_types::TypeId;

use crate::{eval::code::project::Projector, value::InterpValue as V};

pub(super) fn path(
    p: &Projector<'_>,
    ty: TypeId,
    path: &[ValueProjectionStep],
) -> Result<V, String> {
    let element = p.array_element(ty)?;
    path.iter()
        .map(|step| match step {
            ValueProjectionStep::Field(name) => {
                p.variant(element, "Field", vec![V::String(name.clone().into())])
            }
            ValueProjectionStep::TupleElement(position) => {
                p.variant(element, "TupleElement", vec![V::usize(*position as usize)])
            }
            ValueProjectionStep::VariantPayload { variant, position } => p.named_variant(
                element,
                "VariantPayload",
                vec![
                    ("variant", V::String(variant.clone().into())),
                    ("position", V::usize(*position as usize)),
                ],
            ),
            ValueProjectionStep::SequenceElement => {
                p.variant(element, "SequenceElement", Vec::new())
            }
            ValueProjectionStep::ListHead => p.variant(element, "ListHead", Vec::new()),
            ValueProjectionStep::ListTail => p.variant(element, "ListTail", Vec::new()),
            ValueProjectionStep::MapKey => p.variant(element, "MapKey", Vec::new()),
            ValueProjectionStep::MapValue => p.variant(element, "MapValue", Vec::new()),
            ValueProjectionStep::CallResult => p.variant(element, "CallResult", Vec::new()),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|steps| V::Array(steps.into()))
}

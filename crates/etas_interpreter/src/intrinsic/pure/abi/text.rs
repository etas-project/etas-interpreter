use std::borrow::Cow;

use etas_builtin::text::transform::TextOutput;
use etas_types::{PrimitiveType, TypeId};

use crate::value::{ArrayValue, InterpValue, StringValue};

use super::{AbiShape, AdapterError, PureAbiProjector};

pub(in crate::intrinsic::pure) fn from_text_output(
    output: TextOutput<'_>,
    source: &StringValue,
    ty: TypeId,
    projector: &PureAbiProjector,
) -> Result<InterpValue, AdapterError> {
    super::result::restore_wrappers(output, ty, projector, |output, ty, shape| {
        match (shape, output) {
            (AbiShape::Primitive(PrimitiveType::String), TextOutput::String(text)) => {
                let text = match text {
                    Cow::Owned(text) => text.into(),
                    Cow::Borrowed(text)
                        if text.as_ptr() == source.as_ptr() && text.len() == source.len() =>
                    {
                        source.clone()
                    }
                    // A small substring must not retain its entire source allocation.
                    Cow::Borrowed(text) => text.into(),
                };
                Ok(InterpValue::String(text))
            }
            (AbiShape::Array(inner), TextOutput::Array(parts)) => parts
                .map(|part| {
                    from_text_output(
                        TextOutput::String(Cow::Borrowed(part)),
                        source,
                        *inner,
                        projector,
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(ArrayValue::new)
                .map(InterpValue::Array),
            (_, output) => Err(AdapterError::TypeMismatch {
                expected: ty,
                actual: match output {
                    TextOutput::String(_) => "String",
                    TextOutput::Array(_) => "Array",
                }
                .into(),
            }),
        }
    })
}

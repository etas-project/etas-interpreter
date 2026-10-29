use etas_types::{PrimitiveType, TypeId};
use std::ops::Range;

use crate::value::{ArrayValue, InterpValue, StringValue, TextRangeError, TextRangeProjector};

use super::{AbiShape, AdapterError, PureAbiProjector};

pub(in crate::intrinsic::pure) fn from_text_range(
    range: Range<usize>,
    source: &mut TextRangeProjector<'_>,
    ty: TypeId,
    projector: &PureAbiProjector,
) -> Result<InterpValue, AdapterError> {
    super::result::restore_wrappers(range, ty, projector, |range, ty, shape| {
        if !matches!(shape, AbiShape::Primitive(PrimitiveType::String)) {
            return Err(AdapterError::TypeMismatch {
                expected: ty,
                actual: "String".into(),
            });
        }
        source
            .project(range)
            .map(InterpValue::String)
            .map_err(|error| match error {
                TextRangeError::InvalidRange => AdapterError::UnsupportedValue(
                    "text result range does not match checked source".into(),
                ),
                TextRangeError::CapacityExceeded => {
                    AdapterError::Builtin(etas_builtin::BuiltinError::NumericOverflow)
                }
            })
    })
}

pub(in crate::intrinsic::pure) fn from_text_ranges(
    ranges: impl Iterator<Item = Range<usize>>,
    source: &StringValue,
    ty: TypeId,
    projector: &PureAbiProjector,
) -> Result<InterpValue, AdapterError> {
    super::result::restore_wrappers(ranges, ty, projector, |ranges, ty, shape| {
        let AbiShape::Array(inner) = shape else {
            return Err(AdapterError::TypeMismatch {
                expected: ty,
                actual: "Array".into(),
            });
        };
        let mut source = TextRangeProjector::new(source);
        ranges
            .map(|range| from_text_range(range, &mut source, *inner, projector))
            .collect::<Result<Vec<_>, _>>()
            .map(ArrayValue::new)
            .map(InterpValue::Array)
    })
}

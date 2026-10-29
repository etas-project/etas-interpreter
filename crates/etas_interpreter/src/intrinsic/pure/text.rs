use etas_builtin::text::{
    ranges::{line_ranges, split_ranges, trim_range},
    transform::TextTransform,
};

use crate::{intrinsic::dispatch::CheckedPureIntrinsicCall, value::InterpValue};

use super::abi::{
    AdapterError, PureAbiProjector, borrowed,
    text::{from_text_range, from_text_ranges},
};

pub(super) fn execute_join(
    call: &CheckedPureIntrinsicCall,
    args: &[InterpValue],
    projector: &PureAbiProjector,
) -> Result<InterpValue, AdapterError> {
    use super::abi::{AbiShape, input::type_mismatch, output::from_builtin_for_type};
    use etas_builtin::text::join::{JoinError, join_text_projected};
    let [parts, separator] = args else {
        return Err(AdapterError::Arity {
            expected: 2,
            actual: args.len(),
        });
    };
    let (parts, ty) = borrowed::representation_for_type(parts, call.parameter_types[0], projector)?;
    let (Some(AbiShape::Array(inner)), InterpValue::Array(parts)) = (projector.shape(ty), parts)
    else {
        return Err(type_mismatch(ty, parts));
    };
    let parts = parts.borrow();
    let separator =
        match borrowed::shared_string_for_type(separator, call.parameter_types[1], projector) {
            Ok(separator) => separator,
            Err(error) => {
                // Input projection historically reports the first argument's error
                // first. Preserve that ordering without copying its elements.
                for part in parts.iter() {
                    borrowed::shared_string_for_type(part, *inner, projector)?;
                }
                return Err(error);
            }
        };
    let output = join_text_projected(
        parts
            .iter()
            .map(|part| borrowed::shared_string_for_type(part, *inner, projector)),
        separator,
    )
    .map_err(|error| match error {
        JoinError::Projection(error) => error,
        JoinError::OutputTooLarge => {
            AdapterError::Builtin(etas_builtin::BuiltinError::NumericOverflow)
        }
    })?;
    from_builtin_for_type(
        etas_builtin::BuiltinValue::String(output),
        call.result_type,
        projector,
    )
}

pub(super) fn execute_transform(
    transform: TextTransform,
    call: &CheckedPureIntrinsicCall,
    args: &[InterpValue],
    projector: &PureAbiProjector,
) -> Result<InterpValue, AdapterError> {
    if args.len() != transform.arity() {
        return Err(AdapterError::Arity {
            expected: transform.arity(),
            actual: args.len(),
        });
    }
    let source = borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?;
    match transform {
        TextTransform::Trim => from_text_range(
            trim_range(source),
            &mut crate::value::TextRangeProjector::new(source),
            call.result_type,
            projector,
        ),
        TextTransform::Lines => {
            from_text_ranges(line_ranges(source), source, call.result_type, projector)
        }
        TextTransform::Split => {
            let separator =
                borrowed::shared_string_for_type(&args[1], call.parameter_types[1], projector)?;
            let ranges = split_ranges(source, separator).map_err(AdapterError::Builtin)?;
            from_text_ranges(ranges, source, call.result_type, projector)
        }
        TextTransform::Lowercase | TextTransform::Uppercase => {
            // Rust's Unicode case conversion includes cross-scalar context.
            // Keep those semantics instead of mapping chars independently.
            let output = transform
                .evaluate(&[source.as_str()])
                .map_err(AdapterError::Builtin)?;
            super::abi::output::from_builtin_for_type(
                output.into_owned(),
                call.result_type,
                projector,
            )
        }
    }
}

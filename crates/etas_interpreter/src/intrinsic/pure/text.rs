use etas_builtin::text::transform::TextTransform;

use crate::{intrinsic::dispatch::CheckedPureIntrinsicCall, value::InterpValue};

use super::abi::{AdapterError, PureAbiProjector, borrowed, text::from_text_output};

pub(super) fn execute_join(
    call: &CheckedPureIntrinsicCall,
    args: &[InterpValue],
    projector: &PureAbiProjector,
) -> Result<InterpValue, AdapterError> {
    use super::abi::{AbiShape, input::type_mismatch, output::from_builtin_for_type};
    use etas_builtin::text::join::{JoinError, join_projected};
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
    let separator = match borrowed::string_for_type(separator, call.parameter_types[1], projector) {
        Ok(separator) => separator,
        Err(error) => {
            // Input projection historically reports the first argument's error
            // first. Preserve that ordering without copying its elements.
            for part in parts.iter() {
                borrowed::string_for_type(part, *inner, projector)?;
            }
            return Err(error);
        }
    };
    let output = join_projected(
        parts
            .iter()
            .map(|part| borrowed::string_for_type(part, *inner, projector)),
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
    let mut inputs = [source.as_str(), ""];
    if args.len() == 2 {
        inputs[1] = borrowed::string_for_type(&args[1], call.parameter_types[1], projector)?;
    }
    let output = transform
        .evaluate(&inputs[..args.len()])
        .map_err(AdapterError::Builtin)?;
    from_text_output(output, source, call.result_type, projector)
}

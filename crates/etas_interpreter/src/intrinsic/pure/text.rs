use etas_builtin::text::transform::TextTransform;

use crate::{intrinsic::dispatch::CheckedPureIntrinsicCall, value::InterpValue};

use super::abi::{AdapterError, PureAbiProjector, borrowed, text::from_text_output};

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

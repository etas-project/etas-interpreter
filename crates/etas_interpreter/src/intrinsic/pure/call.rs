use etas_builtin::call_pure_intrinsic;

use crate::intrinsic::dispatch::CheckedPureIntrinsicCall;
use crate::value::InterpValue;

use super::abi::input::into_builtin_for_type;
use super::abi::output::from_builtin_for_type;
use super::abi::{AdapterError, PureAbiProjector};
use super::fast_path::{checked_container_fast_path, interpreter_fast_path};

pub fn execute_pure_intrinsic(
    call: &CheckedPureIntrinsicCall,
    args: Vec<InterpValue>,
    projector: &PureAbiProjector,
) -> Result<InterpValue, AdapterError> {
    if args.len() != call.parameter_types.len() {
        return Err(AdapterError::Arity {
            expected: call.parameter_types.len(),
            actual: args.len(),
        });
    }
    if let Some(value) = checked_container_fast_path(call, &args, projector)? {
        return Ok(value);
    }
    if let Some(value) = interpreter_fast_path(call.intrinsic, &args) {
        return from_builtin_for_type(value, call.result_type, projector);
    }
    let builtin_args = args
        .into_iter()
        .zip(call.parameter_types.iter().copied())
        .map(|(value, ty)| into_builtin_for_type(value, ty, projector))
        .collect::<Result<Vec<_>, _>>()?;
    let result =
        call_pure_intrinsic(call.intrinsic, &builtin_args).map_err(AdapterError::Builtin)?;
    from_builtin_for_type(result, call.result_type, projector)
}

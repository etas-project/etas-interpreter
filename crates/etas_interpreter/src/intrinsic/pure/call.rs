use etas_builtin::call_pure_intrinsic;

use crate::intrinsic::dispatch::CheckedPureIntrinsicCall;
use crate::value::InterpValue;

use super::abi::input::into_builtin_for_type;
use super::abi::output::from_builtin_for_type;
use super::abi::{AdapterError, PureAbiProjector};
use super::fast_path::{FastPathResult, checked_container_fast_path, interpreter_fast_path};

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
    if call.intrinsic.0 == etas_std::intrinsic::pure::BYTES_LEN {
        let [value] = args.as_slice() else {
            return Err(AdapterError::Arity {
                expected: 1,
                actual: args.len(),
            });
        };
        let bytes =
            super::abi::borrowed::bytes_for_type(value, call.parameter_types[0], projector)?;
        return from_builtin_for_type(
            etas_builtin::bytes::ops::len_borrowed(bytes),
            call.result_type,
            projector,
        );
    }
    if let Some(query) = etas_builtin::text::query::TextQuery::for_intrinsic(call.intrinsic) {
        if args.len() != query.arity() {
            return Err(AdapterError::Arity {
                expected: query.arity(),
                actual: args.len(),
            });
        }
        let mut borrowed = [""; 2];
        for ((slot, value), ty) in borrowed.iter_mut().zip(&args).zip(&call.parameter_types) {
            *slot = super::abi::borrowed::string_for_type(value, *ty, projector)?;
        }
        let result = query
            .evaluate(&borrowed[..args.len()])
            .map_err(AdapterError::Builtin)?;
        return from_builtin_for_type(result, call.result_type, projector);
    }
    if let Some(transform) =
        etas_builtin::text::transform::TextTransform::for_intrinsic(call.intrinsic)
    {
        return super::text::execute_transform(transform, call, &args, projector);
    }
    let args = match checked_container_fast_path(call, args, projector)? {
        FastPathResult::Value(value) => return Ok(value),
        FastPathResult::Kernel(args) => args,
    };
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

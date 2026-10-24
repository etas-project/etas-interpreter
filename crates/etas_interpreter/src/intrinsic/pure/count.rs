use etas_builtin::collections::count::CountQuery;
use etas_types::PrimitiveType;

use crate::{intrinsic::dispatch::CheckedPureIntrinsicCall, value::InterpValue};

use super::abi::{
    AbiShape, AdapterError, PureAbiProjector, borrowed::representation_for_type,
    input::type_mismatch, output::from_builtin_for_type,
};

pub(super) fn execute_count(
    query: CountQuery,
    call: &CheckedPureIntrinsicCall,
    args: &[InterpValue],
    projector: &PureAbiProjector,
) -> Result<InterpValue, AdapterError> {
    let [value] = args else {
        return Err(AdapterError::Arity {
            expected: 1,
            actual: args.len(),
        });
    };
    let (value, ty) = representation_for_type(value, call.parameter_types[0], projector)?;
    // Counts inspect only the checked container, never materialize its payload.
    let result = match (projector.shape(ty), value) {
        (Some(AbiShape::Array(_)), InterpValue::Array(values)) => {
            query.evaluate_count(values.borrow().len())
        }
        (Some(AbiShape::List(_)), InterpValue::List(values)) => query.evaluate_count(values.len()),
        (Some(AbiShape::Slice(_)), InterpValue::Slice(values)) => {
            query.evaluate_count(values.borrow().len())
        }
        (Some(AbiShape::Map { .. }), InterpValue::Map(values)) => {
            query.evaluate_count(values.borrow().len())
        }
        (Some(AbiShape::Primitive(PrimitiveType::String)), InterpValue::String(text)) => {
            query.evaluate_text(text.as_str())
        }
        _ => return Err(type_mismatch(ty, value)),
    };
    from_builtin_for_type(result, call.result_type, projector)
}

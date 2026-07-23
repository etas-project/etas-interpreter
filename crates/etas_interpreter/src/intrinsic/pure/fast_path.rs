use etas_builtin::{BuiltinValue, collections};
use etas_std::intrinsic;
use etas_types::PrimitiveType;

use crate::intrinsic::dispatch::CheckedPureIntrinsicCall;
use crate::value::InterpValue;

use super::abi::input::type_mismatch;
use super::abi::{AbiShape, AdapterError, PureAbiProjector};

pub(super) fn checked_container_fast_path(
    call: &CheckedPureIntrinsicCall,
    args: &[InterpValue],
    projector: &PureAbiProjector,
) -> Result<Option<InterpValue>, AdapterError> {
    let [arg] = args else {
        return Ok(None);
    };
    match call.intrinsic.0 {
        intrinsic::pure::OPTION_IS_SOME | intrinsic::pure::OPTION_IS_NONE => {
            let Some(AbiShape::Option(_)) = projector.shape(call.parameter_types[0]) else {
                return Err(invalid_checked_abi(call, projector));
            };
            let Some(AbiShape::Primitive(PrimitiveType::Bool)) = projector.shape(call.result_type)
            else {
                return Err(invalid_checked_abi(call, projector));
            };
            let is_some = match arg {
                InterpValue::OptionSome(_) => true,
                InterpValue::OptionNone => false,
                other => return Err(type_mismatch(call.parameter_types[0], other)),
            };
            Ok(Some(InterpValue::Bool(
                if call.intrinsic.0 == intrinsic::pure::OPTION_IS_SOME {
                    is_some
                } else {
                    !is_some
                },
            )))
        }
        intrinsic::pure::RESULT_IS_OK | intrinsic::pure::RESULT_IS_ERR => {
            let Some(AbiShape::Result { .. }) = projector.shape(call.parameter_types[0]) else {
                return Err(invalid_checked_abi(call, projector));
            };
            let Some(AbiShape::Primitive(PrimitiveType::Bool)) = projector.shape(call.result_type)
            else {
                return Err(invalid_checked_abi(call, projector));
            };
            let is_ok = match arg {
                InterpValue::Variant { name, fields }
                    if (name == "Ok" || name == "Err") && fields.len() == 1 =>
                {
                    name == "Ok"
                }
                other => return Err(type_mismatch(call.parameter_types[0], other)),
            };
            Ok(Some(InterpValue::Bool(
                if call.intrinsic.0 == intrinsic::pure::RESULT_IS_OK {
                    is_ok
                } else {
                    !is_ok
                },
            )))
        }
        intrinsic::pure::OPTION_UNWRAP => {
            let Some(AbiShape::Option(inner)) = projector.shape(call.parameter_types[0]) else {
                return Err(invalid_checked_abi(call, projector));
            };
            if *inner != call.result_type {
                return Err(invalid_checked_abi(call, projector));
            }
            match arg {
                InterpValue::OptionSome(value) => Ok(Some((**value).clone())),
                InterpValue::OptionNone => Ok(None),
                other => Err(type_mismatch(call.parameter_types[0], other)),
            }
        }
        intrinsic::pure::RESULT_UNWRAP => {
            let Some(AbiShape::Result { ok, .. }) = projector.shape(call.parameter_types[0]) else {
                return Err(invalid_checked_abi(call, projector));
            };
            if *ok != call.result_type {
                return Err(invalid_checked_abi(call, projector));
            }
            match arg {
                InterpValue::Variant { name, fields } if name == "Ok" && fields.len() == 1 => {
                    Ok(Some(fields[0].clone()))
                }
                InterpValue::Variant { name, fields } if name == "Err" && fields.len() == 1 => {
                    Ok(None)
                }
                other => Err(type_mismatch(call.parameter_types[0], other)),
            }
        }
        intrinsic::pure::OPTION_SOME => {
            let Some(AbiShape::Option(inner)) = projector.shape(call.result_type) else {
                return Err(invalid_checked_abi(call, projector));
            };
            if *inner != call.parameter_types[0] {
                return Err(invalid_checked_abi(call, projector));
            }
            Ok(Some(InterpValue::OptionSome(Box::new(arg.clone()))))
        }
        intrinsic::pure::RESULT_OK | intrinsic::pure::RESULT_ERR => {
            let Some(AbiShape::Result { ok, err }) = projector.shape(call.result_type) else {
                return Err(invalid_checked_abi(call, projector));
            };
            let expected = if call.intrinsic.0 == intrinsic::pure::RESULT_OK {
                *ok
            } else {
                *err
            };
            if expected != call.parameter_types[0] {
                return Err(invalid_checked_abi(call, projector));
            }
            Ok(Some(InterpValue::Variant {
                name: if call.intrinsic.0 == intrinsic::pure::RESULT_OK {
                    "Ok"
                } else {
                    "Err"
                }
                .to_owned(),
                fields: vec![arg.clone()],
            }))
        }
        _ => Ok(None),
    }
}

fn invalid_checked_abi(
    call: &CheckedPureIntrinsicCall,
    projector: &PureAbiProjector,
) -> AdapterError {
    AdapterError::UnsupportedValue(format!(
        "checked ABI for intrinsic {:?} is inconsistent: parameters={:?}, result={:?}",
        call.intrinsic,
        call.parameter_types
            .iter()
            .map(|ty| (*ty, projector.shape(*ty)))
            .collect::<Vec<_>>(),
        (call.result_type, projector.shape(call.result_type))
    ))
}

pub(super) fn interpreter_fast_path(
    intrinsic: etas_std::StdIntrinsicId,
    args: &[InterpValue],
) -> Option<BuiltinValue> {
    match (intrinsic.0, args) {
        (intrinsic::pure::LIST_LEN, [InterpValue::Array(values)]) => {
            Some(collections::list::len_from_count(values.borrow().len()))
        }
        (intrinsic::pure::LIST_LEN, [InterpValue::List(values)]) => {
            Some(collections::list::len_from_count(values.borrow().len()))
        }
        (intrinsic::pure::LIST_LEN, [InterpValue::Slice(values)]) => {
            Some(collections::list::len_from_count(values.borrow().len()))
        }
        (intrinsic::pure::LIST_LEN, [InterpValue::Deque(values)])
        | (intrinsic::pure::LIST_LEN, [InterpValue::Queue(values)])
        | (intrinsic::pure::LIST_LEN, [InterpValue::Stack(values)]) => {
            Some(collections::list::len_from_count(values.borrow().len()))
        }
        (intrinsic::pure::LIST_LEN, [InterpValue::PriorityQueue(entries)])
        | (intrinsic::pure::LIST_LEN, [InterpValue::OrderedMap(entries)]) => {
            Some(collections::list::len_from_count(entries.borrow().len()))
        }
        (intrinsic::pure::LIST_LEN, [InterpValue::OrderedSet(values)]) => {
            Some(collections::list::len_from_count(values.borrow().len()))
        }
        (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::Array(values)]) => Some(
            collections::list::is_empty_from_count(values.borrow().len()),
        ),
        (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::List(values)]) => Some(
            collections::list::is_empty_from_count(values.borrow().len()),
        ),
        (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::Slice(values)]) => Some(
            collections::list::is_empty_from_count(values.borrow().len()),
        ),
        (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::Deque(values)])
        | (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::Queue(values)])
        | (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::Stack(values)]) => Some(
            collections::list::is_empty_from_count(values.borrow().len()),
        ),
        (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::PriorityQueue(entries)])
        | (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::OrderedMap(entries)]) => Some(
            collections::list::is_empty_from_count(entries.borrow().len()),
        ),
        (intrinsic::pure::LIST_IS_EMPTY, [InterpValue::OrderedSet(values)]) => Some(
            collections::list::is_empty_from_count(values.borrow().len()),
        ),
        _ => None,
    }
}

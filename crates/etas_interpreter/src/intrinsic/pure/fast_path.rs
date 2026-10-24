use etas_std::intrinsic;
use etas_types::PrimitiveType;

use crate::intrinsic::dispatch::CheckedPureIntrinsicCall;
use crate::value::InterpValue;

use super::abi::input::type_mismatch;
use super::abi::{AbiShape, AdapterError, PureAbiProjector};

pub(super) enum FastPathResult {
    Value(InterpValue),
    Kernel(Vec<InterpValue>),
}

pub(super) fn checked_container_fast_path(
    call: &CheckedPureIntrinsicCall,
    mut args: Vec<InterpValue>,
    projector: &PureAbiProjector,
) -> Result<FastPathResult, AdapterError> {
    if args.len() != 1
        || !matches!(
            call.intrinsic.0,
            intrinsic::pure::OPTION_IS_SOME
                | intrinsic::pure::OPTION_IS_NONE
                | intrinsic::pure::RESULT_IS_OK
                | intrinsic::pure::RESULT_IS_ERR
                | intrinsic::pure::OPTION_UNWRAP
                | intrinsic::pure::RESULT_UNWRAP
                | intrinsic::pure::OPTION_SOME
                | intrinsic::pure::RESULT_OK
                | intrinsic::pure::RESULT_ERR
        )
    {
        return Ok(FastPathResult::Kernel(args));
    }
    let Some(arg) = args.pop() else {
        return Ok(FastPathResult::Kernel(args));
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
                other => return Err(type_mismatch(call.parameter_types[0], &other)),
            };
            Ok(FastPathResult::Value(InterpValue::Bool(
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
                other => return Err(type_mismatch(call.parameter_types[0], &other)),
            };
            Ok(FastPathResult::Value(InterpValue::Bool(
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
                InterpValue::OptionSome(value) => Ok(FastPathResult::Value(value.into_value())),
                InterpValue::OptionNone => {
                    args.push(InterpValue::OptionNone);
                    Ok(FastPathResult::Kernel(args))
                }
                other => Err(type_mismatch(call.parameter_types[0], &other)),
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
                InterpValue::Variant { name, fields } if name == "Ok" && fields.len() == 1 => Ok(
                    FastPathResult::Value(fields.into_single().ok_or(AdapterError::Arity {
                        expected: 1,
                        actual: 0,
                    })?),
                ),
                InterpValue::Variant { name, fields } if name == "Err" && fields.len() == 1 => {
                    args.push(InterpValue::Variant { name, fields });
                    Ok(FastPathResult::Kernel(args))
                }
                other => Err(type_mismatch(call.parameter_types[0], &other)),
            }
        }
        intrinsic::pure::OPTION_SOME => {
            let Some(AbiShape::Option(inner)) = projector.shape(call.result_type) else {
                return Err(invalid_checked_abi(call, projector));
            };
            if *inner != call.parameter_types[0] {
                return Err(invalid_checked_abi(call, projector));
            }
            Ok(FastPathResult::Value(InterpValue::OptionSome(
                crate::value::SharedValue::new(arg),
            )))
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
            Ok(FastPathResult::Value(InterpValue::Variant {
                name: if call.intrinsic.0 == intrinsic::pure::RESULT_OK {
                    "Ok"
                } else {
                    "Err"
                }
                .to_owned()
                .into(),
                fields: vec![arg].into(),
            }))
        }
        _ => {
            args.push(arg);
            Ok(FastPathResult::Kernel(args))
        }
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

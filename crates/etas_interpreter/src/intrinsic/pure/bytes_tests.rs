use super::*;
use crate::{
    intrinsic::dispatch::CheckedPureIntrinsicCall, testing::allocation::measure, value::InterpValue,
};
use etas_builtin::BuiltinValue;
use etas_std::{StdIntrinsicId, intrinsic::pure};
use etas_types::{NominalTypeRef, PrimitiveType, TrustWrapper, Type, TypeInterner};

#[test]
fn checked_bytes_len_borrows_shared_inputs_without_payload_materialization() {
    let mut types = TypeInterner::new();
    let bytes = types.primitive(PrimitiveType::Bytes);
    let size = types.primitive(PrimitiveType::USize);
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::BYTES_LEN),
        parameter_types: vec![bytes],
        result_type: size,
    };
    for count in [1000, 2000, 4000, 0] {
        let input = InterpValue::Bytes(vec![255; count].into());
        let args = vec![input.clone()];
        let (actual, cost) = measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
        assert_eq!(actual, InterpValue::usize(count));
        assert_eq!(cost.count, 0, "n={count}: {cost:?}");
        assert_eq!(cost.bytes, 0);
    }
}

#[test]
fn owned_bytes_abi_moves_unique_buffers_and_copies_shared_buffers_once() {
    let mut types = TypeInterner::new();
    let bytes = types.primitive(PrimitiveType::Bytes);
    let option = types.intern(Type::Option(bytes));
    let boolean = types.primitive(PrimitiveType::Bool);
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    for count in [1000, 2000, 4000] {
        let input = vec![7; count];
        let pointer = input.as_ptr();
        let input = InterpValue::Bytes(input.into());
        let (kernel, cost) =
            measure(|| abi::input::into_builtin_for_type(input, bytes, &projector).unwrap());
        assert_eq!(cost.count, 0);
        let BuiltinValue::Bytes(buffer) = &kernel else {
            panic!("bytes")
        };
        assert_eq!(buffer.as_ptr(), pointer);
        let (value, cost) =
            measure(|| abi::output::from_builtin_for_type(kernel, bytes, &projector).unwrap());
        assert_eq!(cost.count, 1, "only the shared backing header is allocated");
        assert!(cost.bytes < count);
        let InterpValue::Bytes(buffer) = &value else {
            panic!("bytes")
        };
        assert_eq!(buffer.as_ptr(), pointer);
        let (kernel, cost) = measure(|| {
            abi::input::into_builtin_for_type(value.clone(), bytes, &projector).unwrap()
        });
        assert_eq!(cost.count, 1);
        assert_eq!(cost.bytes, count);
        let BuiltinValue::Bytes(copied) = kernel else {
            panic!("bytes")
        };
        assert_ne!(copied.as_ptr(), pointer);

        let make_some = CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(pure::OPTION_SOME),
            parameter_types: vec![bytes],
            result_type: option,
        };
        let wrapped = execute_pure_intrinsic(&make_some, vec![value], &projector).unwrap();
        let tag = CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(pure::OPTION_IS_SOME),
            parameter_types: vec![option],
            result_type: boolean,
        };
        let args = vec![wrapped.clone()];
        let (actual, cost) = measure(|| execute_pure_intrinsic(&tag, args, &projector).unwrap());
        assert_eq!(actual, InterpValue::Bool(true));
        assert_eq!(cost.bytes, 0);
        let unwrap = CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(pure::OPTION_UNWRAP),
            parameter_types: vec![option],
            result_type: bytes,
        };
        let args = vec![wrapped];
        let (actual, cost) = measure(|| execute_pure_intrinsic(&unwrap, args, &projector).unwrap());
        assert_eq!(cost.bytes, 0);
        let InterpValue::Bytes(buffer) = actual else {
            panic!("bytes")
        };
        assert_eq!(buffer.as_ptr(), pointer);
    }
}

#[test]
fn borrowed_bytes_projection_rejects_invalid_checked_identity_and_payloads() {
    let mut types = TypeInterner::new();
    let bytes = types.primitive(PrimitiveType::Bytes);
    let size = types.primitive(PrimitiveType::USize);
    let boolean = types.primitive(PrimitiveType::Bool);
    let left = types.intern(Type::Nominal(NominalTypeRef {
        name: "LeftBytes".into(),
        representation: Some(bytes),
        params: vec![],
    }));
    let right = types.intern(Type::Nominal(NominalTypeRef {
        name: "RightBytes".into(),
        representation: Some(bytes),
        params: vec![],
    }));
    let trusted = types.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: left,
    });
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::BYTES_LEN),
        parameter_types: vec![trusted],
        result_type: size,
    };
    let wrap = |ty, wrapper, value| InterpValue::Trust {
        wrapper,
        value: crate::value::SharedValue::new(InterpValue::Nominal {
            ty,
            value: crate::value::SharedValue::new(value),
        }),
    };
    let payload = InterpValue::Bytes(vec![0, 128, 255].into());
    let valid = wrap(left, TrustWrapper::Trusted, payload.clone());
    assert_eq!(
        execute_pure_intrinsic(&call, vec![valid.clone()], &projector).unwrap(),
        InterpValue::usize(3)
    );
    for value in [
        payload.clone(),
        wrap(right, TrustWrapper::Trusted, payload.clone()),
        wrap(left, TrustWrapper::Secret, payload),
        wrap(
            left,
            TrustWrapper::Trusted,
            InterpValue::String("wrong".into()),
        ),
    ] {
        assert!(execute_pure_intrinsic(&call, vec![value], &projector).is_err());
    }
    let bad_result = CheckedPureIntrinsicCall {
        result_type: boolean,
        ..call.clone()
    };
    assert!(execute_pure_intrinsic(&bad_result, vec![valid.clone()], &projector).is_err());
    assert!(execute_pure_intrinsic(&call, vec![], &projector).is_err());
    let wrong_arity = CheckedPureIntrinsicCall {
        parameter_types: vec![trusted; 2],
        ..call
    };
    assert!(execute_pure_intrinsic(&wrong_arity, vec![valid.clone(), valid], &projector).is_err());
}

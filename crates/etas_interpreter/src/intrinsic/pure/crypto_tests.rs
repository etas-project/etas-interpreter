use super::*;
use crate::{
    intrinsic::dispatch::CheckedPureIntrinsicCall, testing::allocation::measure, value::InterpValue,
};
use etas_std::{StdIntrinsicId, intrinsic::pure};
use etas_types::{NominalTypeRef, PrimitiveType, TrustWrapper, Type, TypeInterner};

#[test]
fn checked_crypto_queries_do_not_materialize_shared_byte_inputs() {
    let mut types = TypeInterner::new();
    let bytes = types.primitive(PrimitiveType::Bytes);
    let boolean = types.primitive(PrimitiveType::Bool);
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let compare = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::CRYPTO_CONSTANT_TIME_EQ),
        parameter_types: vec![bytes; 2],
        result_type: boolean,
    };
    let digest = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::CRYPTO_SHA256),
        parameter_types: vec![bytes],
        result_type: bytes,
    };
    for size in [1000, 2000, 4000] {
        let input = InterpValue::Bytes(vec![7; size * 1024].into());
        let args = vec![input.clone(), input.clone()];
        let (result, cost) =
            measure(|| execute_pure_intrinsic(&compare, args, &projector).unwrap());
        assert_eq!(result, InterpValue::Bool(true));
        assert_eq!(cost.bytes, 0, "comparison n={size}: {cost:?}");
        let args = vec![input.clone()];
        let (result, cost) = measure(|| execute_pure_intrinsic(&digest, args, &projector).unwrap());
        assert!(matches!(result, InterpValue::Bytes(ref value) if value.len() == 32));
        assert!(cost.bytes <= 128, "digest n={size}: {cost:?}");
        eprintln!("checked shared SHA256 bytes={}: {cost:?}", size * 1024);
        assert!(
            matches!(input, InterpValue::Bytes(ref value) if value.len() == size * 1024 && value[0] == 7)
        );
    }
}

#[test]
fn checked_crypto_validates_wrappers_and_restores_nominal_digest() {
    let mut types = TypeInterner::new();
    let bytes = types.primitive(PrimitiveType::Bytes);
    let boolean = types.primitive(PrimitiveType::Bool);
    let mut nominal = |name: &str| {
        types.intern(Type::Nominal(NominalTypeRef {
            name: name.into(),
            representation: Some(bytes),
            params: vec![],
        }))
    };
    let left = nominal("LeftBytes");
    let right = nominal("RightBytes");
    let digest = nominal("Digest");
    let trusted = types.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: left,
    });
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let wrap = |ty, wrapper, value| InterpValue::Trust {
        wrapper,
        value: crate::value::SharedValue::new(InterpValue::Nominal {
            ty,
            value: crate::value::SharedValue::new(value),
        }),
    };
    let payload = InterpValue::Bytes(b"abc".to_vec().into());
    let valid = wrap(left, TrustWrapper::Trusted, payload.clone());
    for (id, arity, result_type) in [
        (pure::CRYPTO_SHA256, 1, digest),
        (pure::CRYPTO_CONSTANT_TIME_EQ, 2, boolean),
    ] {
        let call = CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(id),
            parameter_types: vec![trusted; arity],
            result_type,
        };
        let result = execute_pure_intrinsic(&call, vec![valid.clone(); arity], &projector).unwrap();
        if arity == 1 {
            let InterpValue::Nominal { ty, value } = result else {
                panic!("digest lost checked identity")
            };
            assert_eq!(ty, digest);
            assert!(matches!(&*value, InterpValue::Bytes(bytes) if bytes.len() == 32));
        } else {
            assert_eq!(result, InterpValue::Bool(true));
        }
        for invalid in [
            payload.clone(),
            wrap(right, TrustWrapper::Trusted, payload.clone()),
            wrap(left, TrustWrapper::Secret, payload.clone()),
            wrap(left, TrustWrapper::Trusted, InterpValue::Bool(true)),
        ] {
            for index in 0..arity {
                let mut args = vec![valid.clone(); arity];
                args[index] = invalid.clone();
                assert!(execute_pure_intrinsic(&call, args, &projector).is_err());
            }
        }
        let wrong_result = CheckedPureIntrinsicCall {
            result_type: if arity == 1 { boolean } else { bytes },
            ..call.clone()
        };
        assert!(
            execute_pure_intrinsic(&wrong_result, vec![valid.clone(); arity], &projector).is_err()
        );
        assert!(execute_pure_intrinsic(&call, vec![], &projector).is_err());
        let wrong_arity = CheckedPureIntrinsicCall {
            parameter_types: vec![trusted; arity + 1],
            ..call
        };
        assert!(
            execute_pure_intrinsic(&wrong_arity, vec![valid.clone(); arity + 1], &projector)
                .is_err()
        );
    }
}

#[test]
fn checked_sha256_unique_input_has_only_fixed_output_allocation() {
    let mut types = TypeInterner::new();
    let bytes = types.primitive(PrimitiveType::Bytes);
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::CRYPTO_SHA256),
        parameter_types: vec![bytes],
        result_type: bytes,
    };
    for size in [1000, 2000, 4000] {
        let args = vec![InterpValue::Bytes(vec![7; size * 1024].into())];
        let (result, cost) = measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
        assert!(matches!(result, InterpValue::Bytes(ref value) if value.len() == 32));
        assert!(cost.bytes <= 128, "n={size}: {cost:?}");
    }
}

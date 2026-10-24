use super::*;
use crate::{
    intrinsic::dispatch::CheckedPureIntrinsicCall, testing::allocation::measure, value::InterpValue,
};
use etas_std::{StdIntrinsicId, intrinsic::pure};
use etas_types::{NominalTypeRef, PrimitiveType, TrustWrapper, Type, TypeInterner};

#[test]
fn checked_text_queries_borrow_shared_inputs_without_payload_materialization() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let boolean = types.primitive(PrimitiveType::Bool);
    let size = types.primitive(PrimitiveType::USize);
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    for count in [1000, 2000, 4000] {
        let text = InterpValue::String(format!("start{}end", "中😀e\u{301}".repeat(count)).into());
        for (id, needle, result_type, expected) in [
            (
                pure::TEXT_CONTAINS,
                Some("😀e"),
                boolean,
                InterpValue::Bool(true),
            ),
            (
                pure::TEXT_CONTAINS,
                Some("absent"),
                boolean,
                InterpValue::Bool(false),
            ),
            (
                pure::TEXT_STARTS_WITH,
                Some("start"),
                boolean,
                InterpValue::Bool(true),
            ),
            (
                pure::TEXT_ENDS_WITH,
                Some("end"),
                boolean,
                InterpValue::Bool(true),
            ),
            (
                pure::TEXT_LEN,
                None,
                size,
                InterpValue::usize(count * 4 + 8),
            ),
        ] {
            let args = std::iter::once(text.clone())
                .chain(needle.map(|s| InterpValue::String(s.into())))
                .collect::<Vec<_>>();
            let call = CheckedPureIntrinsicCall {
                intrinsic: StdIntrinsicId(id),
                parameter_types: vec![string; args.len()],
                result_type,
            };
            let (actual, allocations) =
                measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
            assert_eq!(actual, expected);
            assert_eq!(
                allocations.count, 0,
                "intrinsic={id} n={count}: {allocations:?}"
            );
            assert_eq!(allocations.bytes, 0);
        }
    }
}

#[test]
fn borrowed_text_projection_validates_nominal_trust_and_result_identity() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let boolean = types.primitive(PrimitiveType::Bool);
    let left = types.intern(Type::Nominal(NominalTypeRef {
        name: "Left".into(),
        representation: Some(string),
        params: vec![],
    }));
    let right = types.intern(Type::Nominal(NominalTypeRef {
        name: "Right".into(),
        representation: Some(string),
        params: vec![],
    }));
    let trusted = types.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: left,
    });
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::TEXT_CONTAINS),
        parameter_types: vec![trusted, string],
        result_type: boolean,
    };
    let valid = InterpValue::Trust {
        wrapper: TrustWrapper::Trusted,
        value: crate::value::SharedValue::new(InterpValue::Nominal {
            ty: left,
            value: crate::value::SharedValue::new(InterpValue::String("valid".into())),
        }),
    };
    assert_eq!(
        execute_pure_intrinsic(
            &call,
            vec![valid.clone(), InterpValue::String("ali".into())],
            &projector
        )
        .unwrap(),
        InterpValue::Bool(true)
    );
    for value in [
        InterpValue::String("valid".into()),
        InterpValue::Trust {
            wrapper: TrustWrapper::Secret,
            value: crate::value::SharedValue::new(InterpValue::Nominal {
                ty: left,
                value: crate::value::SharedValue::new(InterpValue::String("valid".into())),
            }),
        },
        InterpValue::Trust {
            wrapper: TrustWrapper::Trusted,
            value: crate::value::SharedValue::new(InterpValue::Nominal {
                ty: right,
                value: crate::value::SharedValue::new(InterpValue::String("valid".into())),
            }),
        },
        InterpValue::Trust {
            wrapper: TrustWrapper::Trusted,
            value: crate::value::SharedValue::new(InterpValue::Nominal {
                ty: left,
                value: crate::value::SharedValue::new(InterpValue::Bool(true)),
            }),
        },
    ] {
        assert!(
            execute_pure_intrinsic(
                &call,
                vec![value, InterpValue::String("ali".into())],
                &projector
            )
            .is_err()
        );
    }
    let bad_result = CheckedPureIntrinsicCall {
        result_type: string,
        ..call.clone()
    };
    assert!(
        execute_pure_intrinsic(
            &bad_result,
            vec![valid, InterpValue::String("ali".into())],
            &projector
        )
        .is_err()
    );
    assert!(execute_pure_intrinsic(&call, vec![], &projector).is_err());
}

#[test]
fn malformed_text_query_diagnostics_do_not_render_sensitive_payloads() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let size = types.primitive(PrimitiveType::USize);
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::TEXT_LEN),
        parameter_types: vec![string],
        result_type: size,
    };
    let payload = "secret-payload-sentinel".repeat(4096);
    let value = InterpValue::Trust {
        wrapper: TrustWrapper::Secret,
        value: crate::value::SharedValue::new(InterpValue::String(payload.into())),
    };
    let (error, cost) =
        measure(|| execute_pure_intrinsic(&call, vec![value], &projector).unwrap_err());
    assert!(!format!("{error:?}").contains("secret-payload-sentinel"));
    assert!(
        cost.bytes < 1024,
        "diagnostic must not format payload: {cost:?}"
    );
}

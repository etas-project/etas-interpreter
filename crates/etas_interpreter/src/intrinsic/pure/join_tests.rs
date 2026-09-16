use super::*;
use crate::{
    intrinsic::dispatch::CheckedPureIntrinsicCall,
    testing::allocation::measure,
    value::{ArrayValue, InterpValue},
};
use etas_std::{StdIntrinsicId, intrinsic::pure};
use etas_types::{PrimitiveType, Type, TypeInterner};

#[test]
fn join_wrapped_elements_reuses_prepared_projection_without_per_element_allocations() {
    use crate::value::SharedValue;
    use etas_types::{NominalTypeRef, TrustWrapper};
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let nominal = types.intern(Type::Nominal(NominalTypeRef {
        name: "Text".into(),
        params: vec![],
        representation: Some(string),
    }));
    let trusted = types.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: nominal,
    });
    let array = types.intern(Type::Array(trusted));
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::TEXT_JOIN),
        parameter_types: vec![array, string],
        result_type: string,
    };
    for count in [1000, 2000, 4000] {
        let payload = "x".repeat(1024);
        let item = InterpValue::Trust {
            wrapper: TrustWrapper::Trusted,
            value: SharedValue::new(InterpValue::Nominal {
                ty: nominal,
                value: SharedValue::new(InterpValue::String(payload.into())),
            }),
        };
        let input = InterpValue::Array(ArrayValue::new(vec![item; count]));
        let args = vec![input.clone(), InterpValue::String("|".into())];
        let (output, cost) = measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
        let bytes = count * 1024 + count - 1;
        assert!(
            cost.count <= 2,
            "wrapper projection allocated per element: n={count}, {cost:?}"
        );
        assert!(
            cost.bytes <= bytes + 128,
            "intermediate allocation: {cost:?}"
        );
        let InterpValue::String(output) = output else {
            panic!("string")
        };
        assert_eq!(output.len(), bytes);
        assert_eq!(output.split('|').count(), count);
    }
}

#[test]
fn join_borrows_shared_input_and_allocates_only_the_rendered_output() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let array = types.intern(Type::Array(string));
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::TEXT_JOIN),
        parameter_types: vec![array, string],
        result_type: string,
    };
    for count in [1000, 2000, 4000] {
        let input = InterpValue::Array(ArrayValue::new(
            (0..count)
                .map(|_| InterpValue::String("x".repeat(1024).into()))
                .collect(),
        ));
        let args = vec![input.clone(), InterpValue::String("|".into())];
        let (output, cost) = measure(|| execute_pure_intrinsic(&call, args, &projector).unwrap());
        let expected_bytes = count * 1024 + count - 1;
        eprintln!("shared text join n={count}: {cost:?}, output={expected_bytes}");
        assert!(cost.bytes <= expected_bytes + 128, "copied input: {cost:?}");
        assert!(cost.count <= 2, "intermediate materialization: {cost:?}");
        let InterpValue::String(text) = output else {
            panic!("string")
        };
        assert_eq!(text.len(), expected_bytes);
        assert_eq!(text.split('|').count(), count);
        let InterpValue::Array(values) = input else {
            panic!("array")
        };
        assert_eq!(values.borrow().len(), count);
    }
}

fn previous_projection(
    call: &CheckedPureIntrinsicCall,
    args: Vec<InterpValue>,
    projector: &PureAbiProjector,
) -> Result<InterpValue, AdapterError> {
    let args = args
        .into_iter()
        .zip(&call.parameter_types)
        .map(|(value, ty)| abi::input::into_builtin_for_type(value, *ty, projector))
        .collect::<Result<Vec<_>, _>>()?;
    let value =
        etas_builtin::call_pure_intrinsic(call.intrinsic, &args).map_err(AdapterError::Builtin)?;
    abi::output::from_builtin_for_type(value, call.result_type, projector)
}

#[test]
fn join_preserves_checked_wrapper_validation_and_unicode_results() {
    use crate::value::SharedValue;
    use etas_types::{NominalTypeRef, TrustWrapper};
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let nominal = types.intern(Type::Nominal(NominalTypeRef {
        name: "Text".into(),
        params: vec![],
        representation: Some(string),
    }));
    let other = types.intern(Type::Nominal(NominalTypeRef {
        name: "Other".into(),
        params: vec![],
        representation: Some(string),
    }));
    let trusted = types.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: nominal,
    });
    let array = types.intern(Type::Array(trusted));
    let container = types.intern(Type::Nominal(NominalTypeRef {
        name: "Texts".into(),
        params: vec![],
        representation: Some(array),
    }));
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::TEXT_JOIN),
        parameter_types: vec![container, trusted],
        result_type: trusted,
    };
    let wrap = |value, ty, wrapper| InterpValue::Trust {
        wrapper,
        value: SharedValue::new(InterpValue::Nominal {
            ty,
            value: SharedValue::new(value),
        }),
    };
    let text = |s: &str| {
        wrap(
            InterpValue::String(s.into()),
            nominal,
            TrustWrapper::Trusted,
        )
    };
    let pack = |parts| InterpValue::Nominal {
        ty: container,
        value: SharedValue::new(InterpValue::Array(ArrayValue::new(parts))),
    };
    for parts in [
        vec![],
        vec![text("")],
        vec![text("中"), text("😀"), text("e\u{301}")],
        vec![text(""), text("a"), text("")],
    ] {
        for sep in ["", "|", "😀"] {
            let args = vec![pack(parts.clone()), text(sep)];
            let expected = previous_projection(&call, args.clone(), &projector).unwrap();
            assert_eq!(
                execute_pure_intrinsic(&call, args, &projector).unwrap(),
                expected
            );
        }
    }
    let bad_parts = [
        InterpValue::String("unwrapped".into()),
        wrap(
            InterpValue::String("wrong nominal".into()),
            other,
            TrustWrapper::Trusted,
        ),
        wrap(
            InterpValue::String("wrong trust".into()),
            nominal,
            TrustWrapper::Untrusted,
        ),
        wrap(InterpValue::Bool(false), nominal, TrustWrapper::Trusted),
    ];
    for bad in bad_parts {
        let args = vec![pack(vec![text("first"), bad]), text(",")];
        let expected = previous_projection(&call, args.clone(), &projector).unwrap_err();
        assert_eq!(
            execute_pure_intrinsic(&call, args, &projector).unwrap_err(),
            expected
        );
    }
    for args in [
        vec![InterpValue::Array(ArrayValue::new(vec![])), text(",")],
        vec![pack(vec![text("a")]), InterpValue::Bool(false)],
        vec![
            pack(vec![InterpValue::Bool(true)]),
            InterpValue::Bool(false),
        ],
    ] {
        assert_eq!(
            execute_pure_intrinsic(&call, args.clone(), &projector),
            previous_projection(&call, args, &projector)
        );
    }
    let bad_result = CheckedPureIntrinsicCall {
        result_type: array,
        ..call
    };
    let args = vec![pack(vec![text("a")]), text(",")];
    assert_eq!(
        execute_pure_intrinsic(&bad_result, args.clone(), &projector),
        previous_projection(&bad_result, args, &projector)
    );
}

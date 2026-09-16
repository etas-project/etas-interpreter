use etas_std::{StdIntrinsicId, intrinsic::pure};
use etas_types::{NominalTypeRef, PrimitiveType, TrustWrapper, Type, TypeId, TypeInterner};

use crate::{
    intrinsic::dispatch::CheckedPureIntrinsicCall,
    testing::allocation::measure,
    value::{ArrayValue, InterpValue, ListValue, MapValue, SharedValue, SliceValue},
};

use super::{AdapterError, PureAbiProjector, execute_pure_intrinsic};

#[test]
fn count_rejects_runtime_shape_that_disagrees_with_checked_parameter() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let boolean = types.primitive(PrimitiveType::Bool);
    let size = types.primitive(PrimitiveType::USize);
    let array = types.intern(Type::Array(string));
    let nominal = types.intern(Type::Nominal(NominalTypeRef {
        name: "Texts".into(),
        params: vec![],
        representation: Some(array),
    }));
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    for parameter in [boolean, nominal] {
        for (id, result_type) in [(pure::LIST_LEN, size), (pure::LIST_IS_EMPTY, boolean)] {
            let call = CheckedPureIntrinsicCall {
                intrinsic: StdIntrinsicId(id),
                parameter_types: vec![parameter],
                result_type,
            };
            let result = execute_pure_intrinsic(
                &call,
                vec![InterpValue::Array(ArrayValue::new(vec![]))],
                &projector,
            );
            assert!(
                result.is_err(),
                "unchecked shape accepted: {call:?}: {result:?}"
            );
        }
    }
}

#[test]
fn global_string_count_borrows_unicode_text_without_payload_materialization() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let boolean = types.primitive(PrimitiveType::Bool);
    let size = types.primitive(PrimitiveType::USize);
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    for count in [0, 1000, 2000, 4000] {
        let input = InterpValue::String("中😀e\u{301}".repeat(count).into());
        for (id, result_type, expected) in [
            (pure::LIST_LEN, size, InterpValue::usize(count * 4)),
            (pure::LIST_IS_EMPTY, boolean, InterpValue::Bool(count == 0)),
        ] {
            let call = CheckedPureIntrinsicCall {
                intrinsic: StdIntrinsicId(id),
                parameter_types: vec![string],
                result_type,
            };
            let args = vec![input.clone()];
            let (result, cost) = measure(|| execute_pure_intrinsic(&call, args, &projector));
            assert_eq!(result.unwrap(), expected);
            assert_eq!(
                (cost.count, cost.bytes),
                (0, 0),
                "n={count}, id={id}: {cost:?}"
            );
        }
    }
}

#[test]
fn count_queries_borrow_all_supported_container_payloads() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let boolean = types.primitive(PrimitiveType::Bool);
    let size = types.primitive(PrimitiveType::USize);
    let array = types.intern(Type::Array(string));
    let list = types.intern(Type::List(string));
    let slice = types.intern(Type::Slice(string));
    let map = types.intern(Type::Map {
        key: string,
        value: string,
    });
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    for count in [0, 1000, 2000, 4000] {
        let item = InterpValue::String("x".repeat(1024).into());
        let backing = ArrayValue::new(vec![item.clone(); count + 2]);
        let values = [
            (
                array,
                InterpValue::Array(ArrayValue::new(vec![item.clone(); count])),
            ),
            (
                list,
                InterpValue::List(ListValue::new(vec![item.clone(); count])),
            ),
            (
                slice,
                InterpValue::Slice(SliceValue::from_array(backing.clone(), 1..count + 1).unwrap()),
            ),
            (
                map,
                InterpValue::Map(MapValue::new(
                    (0..count)
                        .map(|i| (InterpValue::String(i.to_string().into()), item.clone()))
                        .collect(),
                )),
            ),
        ];
        for (ty, value) in &values {
            for (id, result_type, expected) in [
                (pure::LIST_LEN, size, InterpValue::usize(count)),
                (pure::LIST_IS_EMPTY, boolean, InterpValue::Bool(count == 0)),
            ] {
                let call = CheckedPureIntrinsicCall {
                    intrinsic: StdIntrinsicId(id),
                    parameter_types: vec![*ty],
                    result_type,
                };
                let args = vec![value.clone()];
                let (result, cost) = measure(|| execute_pure_intrinsic(&call, args, &projector));
                assert_eq!(result.unwrap(), expected);
                assert_eq!(
                    (cost.count, cost.bytes),
                    (0, 0),
                    "n={count}, ty={ty:?}: {cost:?}"
                );
                // A valid shape for another countable type cannot substitute for this type.
                for (other_ty, other) in &values {
                    if other_ty != ty {
                        assert!(matches!(
                            execute_pure_intrinsic(&call, vec![other.clone()], &projector),
                            Err(AdapterError::TypeMismatch { .. })
                        ));
                    }
                }
            }
        }
        assert_eq!(backing.borrow().len(), count + 2);
    }
}

#[test]
fn count_projection_preserves_checked_wrappers_and_result_identity() {
    use super::abi::{input::into_builtin_for_type, output::from_builtin_for_type};
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let boolean = types.primitive(PrimitiveType::Bool);
    let size = types.primitive(PrimitiveType::USize);
    let array = types.intern(Type::Array(string));
    let nominal = types.intern(Type::Nominal(NominalTypeRef {
        name: "Texts".into(),
        params: vec![],
        representation: Some(array),
    }));
    let trusted = types.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: nominal,
    });
    let count_type = types.intern(Type::Nominal(NominalTypeRef {
        name: "Count".into(),
        params: vec![],
        representation: Some(size),
    }));
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let input = InterpValue::Trust {
        wrapper: TrustWrapper::Trusted,
        value: SharedValue::new(InterpValue::Nominal {
            ty: nominal,
            value: SharedValue::new(InterpValue::Array(ArrayValue::new(vec![
                InterpValue::String("x".into()),
            ]))),
        }),
    };
    for (id, result_type) in [(pure::LIST_LEN, count_type), (pure::LIST_IS_EMPTY, boolean)] {
        let call = CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(id),
            parameter_types: vec![trusted],
            result_type,
        };
        let expected = etas_builtin::call_pure_intrinsic(
            call.intrinsic,
            &[into_builtin_for_type(input.clone(), trusted, &projector).unwrap()],
        )
        .unwrap();
        let expected = from_builtin_for_type(expected, result_type, &projector).unwrap();
        assert_eq!(
            execute_pure_intrinsic(&call, vec![input.clone()], &projector).unwrap(),
            expected
        );
        let InterpValue::Trust { value, .. } = input.clone() else {
            unreachable!()
        };
        let wrong_tag = InterpValue::Trust {
            wrapper: TrustWrapper::Untrusted,
            value,
        };
        assert!(
            matches!(execute_pure_intrinsic(&call, vec![wrong_tag], &projector), Err(AdapterError::TypeMismatch { expected, .. }) if expected == trusted)
        );
        let wrong_identity = InterpValue::Trust {
            wrapper: TrustWrapper::Trusted,
            value: SharedValue::new(InterpValue::Nominal {
                ty: count_type,
                value: SharedValue::new(InterpValue::Array(ArrayValue::new(vec![]))),
            }),
        };
        assert_eq!(
            execute_pure_intrinsic(&call, vec![wrong_identity], &projector),
            Err(AdapterError::NominalIdentity {
                expected: nominal,
                actual: count_type
            })
        );
    }
}

#[test]
fn count_query_rejects_incomplete_or_unsupported_checked_abi() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let size = types.primitive(PrimitiveType::USize);
    let array = types.intern(Type::Array(string));
    let unresolved = types.intern(Type::Named(etas_types::NamedTypeRef {
        name: "std.Unresolved".into(),
    }));
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(pure::LIST_LEN),
        parameter_types: vec![array],
        result_type: size,
    };
    let value = InterpValue::Array(ArrayValue::new(vec![]));
    for ty in [TypeId(999), unresolved] {
        assert!(
            execute_pure_intrinsic(
                &CheckedPureIntrinsicCall {
                    parameter_types: vec![ty],
                    ..call.clone()
                },
                vec![value.clone()],
                &projector
            )
            .is_err()
        );
    }
    for result_type in [TypeId(999), unresolved, string] {
        assert!(
            execute_pure_intrinsic(
                &CheckedPureIntrinsicCall {
                    result_type,
                    ..call.clone()
                },
                vec![value.clone()],
                &projector
            )
            .is_err()
        );
    }
    for parameter_types in [vec![], vec![array, array]] {
        let args = vec![value.clone(); parameter_types.len()];
        assert!(matches!(
            execute_pure_intrinsic(
                &CheckedPureIntrinsicCall {
                    parameter_types,
                    ..call.clone()
                },
                args,
                &projector
            ),
            Err(AdapterError::Arity { expected: 1, .. })
        ));
    }
    for value in [
        InterpValue::Deque(crate::value::DequeValue::new(vec![])),
        InterpValue::Bool(false),
    ] {
        assert!(execute_pure_intrinsic(&call, vec![value], &projector).is_err());
    }
}

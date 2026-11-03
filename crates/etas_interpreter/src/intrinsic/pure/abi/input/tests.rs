use super::*;
use crate::{testing::allocation::measure, value::SharedValue};
use etas_types::{NominalTypeRef, RefinementId, TrustWrapper, Type, TypeStore};

fn wrapped_text(depth: usize) -> (TypeStore, TypeId, InterpValue, *const u8) {
    let mut store = TypeStore::new();
    let mut ty = store.intern(Type::Primitive(PrimitiveType::String));
    let text = "payload".repeat(1024);
    let pointer = text.as_ptr();
    let mut value = InterpValue::String(text.into());
    for layer in 0..depth {
        match layer % 3 {
            0 => {
                ty = store.intern(Type::Trust {
                    wrapper: TrustWrapper::Public,
                    inner: ty,
                });
                value = InterpValue::Trust {
                    wrapper: TrustWrapper::Public,
                    value: SharedValue::new(value),
                };
            }
            1 => {
                ty = store.intern(Type::Nominal(NominalTypeRef {
                    read_only_erased_view: false,
                    name: format!("Layer{layer}"),
                    params: vec![],
                    representation: Some(ty),
                }));
                value = InterpValue::Nominal {
                    ty,
                    value: SharedValue::new(value),
                };
            }
            _ => {
                ty = store.intern(Type::Refined {
                    base: ty,
                    predicate: RefinementId(layer as u32),
                });
            }
        }
    }
    (store, ty, value, pointer)
}

#[test]
fn owned_wrapper_input_projection_is_stack_safe_and_moves_the_unique_payload() {
    if crate::testing::in_subprocess("ETAS_TEST_OWNED_ABI_WRAPPERS") {
        return;
    }
    for depth in [1000, 2000, 4000, 30_000] {
        let (store, ty, value, pointer) = wrapped_text(depth);
        let projector = PureAbiProjector::build(&store, &Default::default()).unwrap();
        let (output, cost) = measure(|| into_builtin_for_type(value, ty, &projector).unwrap());
        let BuiltinValue::String(text) = output else {
            panic!("string ABI output")
        };
        assert_eq!(text, "payload".repeat(1024));
        assert_eq!(text.as_ptr(), pointer);
        assert_eq!(cost.count, 0, "depth={depth}: {cost:?}");
        eprintln!("owned wrapper ABI depth={depth}: {cost:?}");
        let (restored, result_cost) = measure(|| {
            super::super::output::from_builtin_for_type(BuiltinValue::String(text), ty, &projector)
                .unwrap()
        });
        let leaf =
            super::super::borrowed::shared_string_for_type(&restored, ty, &projector).unwrap();
        assert_eq!(leaf.as_ptr(), pointer);
        eprintln!("required restored wrapper nodes depth={depth}: {result_cost:?}");
    }
}

#[test]
fn owned_wrapper_input_rejects_deep_nominal_identity_without_copying_or_leaking_payload() {
    if crate::testing::in_subprocess("ETAS_TEST_INVALID_OWNED_ABI_WRAPPERS") {
        return;
    }
    for depth in [0, 1000, 4000, 30_000] {
        let mut store = TypeStore::new();
        let string = store.intern(Type::Primitive(PrimitiveType::String));
        let expected = store.intern(Type::Nominal(NominalTypeRef {
            read_only_erased_view: false,
            name: "Expected".into(),
            params: vec![],
            representation: Some(string),
        }));
        let actual = store.intern(Type::Nominal(NominalTypeRef {
            read_only_erased_view: false,
            name: "Actual".into(),
            params: vec![],
            representation: Some(string),
        }));
        let mut ty = expected;
        let mut value = InterpValue::Nominal {
            ty: actual,
            value: SharedValue::new(InterpValue::String("SECRET_MARKER".repeat(1024).into())),
        };
        for _ in 0..depth {
            ty = store.intern(Type::Trust {
                wrapper: TrustWrapper::Public,
                inner: ty,
            });
            value = InterpValue::Trust {
                wrapper: TrustWrapper::Public,
                value: SharedValue::new(value),
            };
        }
        let projector = PureAbiProjector::build(&store, &Default::default()).unwrap();
        let (result, cost) = measure(|| into_builtin_for_type(value.clone(), ty, &projector));
        let error = result.unwrap_err();
        assert_eq!(error, AdapterError::NominalIdentity { expected, actual });
        assert!(!format!("{error:?}").contains("SECRET_MARKER"));
        assert_eq!(cost.count, 0, "depth={depth}: {cost:?}");
    }
}

#[test]
fn owned_wrapper_input_preserves_cycle_missing_type_and_trust_failures() {
    let mut store = TypeStore::new();
    let cycle = store.intern(Type::Refined {
        base: TypeId(0),
        predicate: RefinementId(0),
    });
    let missing = TypeId(99);
    let missing_inner = store.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: missing,
    });
    let projector = PureAbiProjector::build(&store, &Default::default()).unwrap();
    assert_eq!(
        into_builtin_for_type(InterpValue::Unit, cycle, &projector),
        Err(AdapterError::UnsupportedValue(
            "cyclic checked input ABI".into()
        ))
    );
    assert_eq!(
        into_builtin_for_type(
            InterpValue::Trust {
                wrapper: TrustWrapper::Trusted,
                value: InterpValue::Unit.into(),
            },
            missing_inner,
            &projector
        ),
        Err(AdapterError::MissingType(missing))
    );
    let invalid = InterpValue::Trust {
        wrapper: TrustWrapper::Secret,
        value: InterpValue::String("SECRET_MARKER".repeat(1024).into()).into(),
    };
    let error = into_builtin_for_type(invalid, missing_inner, &projector).unwrap_err();
    assert!(
        matches!(error, AdapterError::TypeMismatch { expected, .. } if expected == missing_inner)
    );
    assert!(!format!("{error:?}").contains("SECRET_MARKER"));
}

#[test]
fn shared_wrapper_input_copies_only_the_required_final_owned_text() {
    if crate::testing::in_subprocess("ETAS_TEST_SHARED_ABI_WRAPPERS") {
        return;
    }
    for depth in [1000, 2000, 4000] {
        let (store, ty, value, pointer) = wrapped_text(depth);
        let projector = PureAbiProjector::build(&store, &Default::default()).unwrap();
        let (output, cost) =
            measure(|| into_builtin_for_type(value.clone(), ty, &projector).unwrap());
        let BuiltinValue::String(text) = output else {
            panic!("string ABI output")
        };
        assert_eq!(text, "payload".repeat(1024));
        assert_ne!(text.as_ptr(), pointer);
        assert_eq!(cost.count, 1, "depth={depth}: {cost:?}");
        assert_eq!(cost.bytes, text.len(), "depth={depth}: {cost:?}");
        assert_eq!(
            super::super::borrowed::shared_string_for_type(&value, ty, &projector)
                .unwrap()
                .as_ptr(),
            pointer
        );
        eprintln!("shared wrapper ABI depth={depth}: {cost:?}");
    }
}

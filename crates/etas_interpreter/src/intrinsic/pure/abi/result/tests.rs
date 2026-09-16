use super::*;
use crate::testing::allocation::measure;
use etas_types::{PrimitiveType, RefinementId, TrustWrapper, Type, TypeStore};

#[test]
fn result_wrapper_walk_restores_nominal_and_trust_order() {
    let mut store = TypeStore::new();
    let boolean = store.intern(Type::Primitive(PrimitiveType::Bool));
    let inner = store.intern(Type::Nominal(etas_types::NominalTypeRef {
        name: "Inner".into(),
        params: vec![],
        representation: Some(boolean),
    }));
    let trusted = store.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner,
    });
    let refined = store.intern(Type::Refined {
        base: trusted,
        predicate: RefinementId(0),
    });
    let outer = store.intern(Type::Nominal(etas_types::NominalTypeRef {
        name: "Outer".into(),
        params: vec![],
        representation: Some(refined),
    }));
    let projector = PureAbiProjector::build(&store).unwrap();
    let result = restore_wrappers(true, outer, &projector, |value, ty, _| {
        assert_eq!(ty, boolean);
        Ok(InterpValue::Bool(value))
    })
    .unwrap();
    assert_eq!(
        result,
        InterpValue::Nominal {
            ty: outer,
            value: InterpValue::Trust {
                wrapper: TrustWrapper::Trusted,
                value: InterpValue::Nominal {
                    ty: inner,
                    value: InterpValue::Bool(true).into(),
                }
                .into(),
            }
            .into(),
        }
    );
}

#[test]
fn result_refinement_walk_does_not_allocate_per_call() {
    for depth in [1000, 2000, 4000, 30_000] {
        let mut store = TypeStore::new();
        let boolean = store.intern(Type::Primitive(PrimitiveType::Bool));
        let mut root = boolean;
        for id in 0..depth {
            root = store.intern(Type::Refined {
                base: root,
                predicate: RefinementId(id),
            });
        }
        let projector = PureAbiProjector::build(&store).unwrap();
        let (result, cost) = measure(|| {
            restore_wrappers(true, root, &projector, |value, ty, shape| {
                assert_eq!(ty, boolean);
                assert_eq!(*shape, AbiShape::Primitive(PrimitiveType::Bool));
                Ok(InterpValue::Bool(value))
            })
            .unwrap()
        });
        assert_eq!(result, InterpValue::Bool(true));
        assert_eq!(cost.count, 0, "depth={depth}: {cost:?}");
        assert_eq!(cost.bytes, 0, "depth={depth}: {cost:?}");
    }
}

#[test]
fn result_wrapper_walk_rejects_cycles_before_invoking_leaf() {
    for trust in [false, true] {
        let mut store = TypeStore::new();
        let cyclic = store.intern(if trust {
            Type::Trust {
                wrapper: TrustWrapper::Trusted,
                inner: TypeId(0),
            }
        } else {
            Type::Refined {
                base: TypeId(0),
                predicate: RefinementId(0),
            }
        });
        let root = store.intern(Type::Refined {
            base: cyclic,
            predicate: RefinementId(1),
        });
        let projector = PureAbiProjector::build(&store).unwrap();
        assert_eq!(
            restore_wrappers((), root, &projector, |_, _, _| panic!("cyclic ABI leaf")),
            Err(AdapterError::UnsupportedValue(
                "cyclic checked result ABI".into()
            ))
        );
    }
}

#[test]
fn result_wrapper_walk_preserves_missing_type_and_leaf_errors() {
    let mut store = TypeStore::new();
    let missing = TypeId(99);
    let root = store.intern(Type::Refined {
        base: missing,
        predicate: RefinementId(0),
    });
    let boolean = store.intern(Type::Primitive(PrimitiveType::Bool));
    let trusted = store.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: boolean,
    });
    let projector = PureAbiProjector::build(&store).unwrap();
    for ty in [root, missing] {
        assert_eq!(
            restore_wrappers((), ty, &projector, |_, _, _| panic!("missing ABI leaf")),
            Err(AdapterError::MissingType(missing))
        );
    }
    let failure = AdapterError::TypeMismatch {
        expected: boolean,
        actual: "String".into(),
    };
    assert_eq!(
        restore_wrappers((), trusted, &projector, |_, _, _| Err(failure.clone())),
        Err(failure)
    );
}

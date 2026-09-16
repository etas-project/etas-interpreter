use super::*;
use crate::value::SharedValue;
use etas_types::{RefinementId, TrustWrapper, Type, TypeStore};

#[test]
fn borrowed_projection_handles_deep_refinement_without_query_allocations() {
    let mut store = TypeStore::new();
    let string = store.intern(Type::Primitive(PrimitiveType::String));
    let mut ty = string;
    for depth in 0..30_000 {
        ty = store.intern(Type::Refined {
            base: ty,
            predicate: RefinementId(depth),
        });
    }
    let projector = PureAbiProjector::build(&store).unwrap();
    let value = InterpValue::String("中😀e\u{301}".into());
    let (text, cost) =
        crate::testing::allocation::measure(|| string_for_type(&value, ty, &projector).unwrap());
    assert_eq!(text, "中😀e\u{301}");
    assert_eq!(cost.count, 0);
    assert_eq!(cost.bytes, 0);
}

#[test]
fn borrowed_projection_rejects_cycles_after_validating_entered_wrappers() {
    let mut store = TypeStore::new();
    let cyclic = store.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: TypeId(0),
    });
    let root = store.intern(Type::Refined {
        base: cyclic,
        predicate: RefinementId(0),
    });
    let projector = PureAbiProjector::build(&store).unwrap();
    let value = InterpValue::Trust {
        wrapper: TrustWrapper::Trusted,
        value: SharedValue::new(InterpValue::String("end".into())),
    };
    assert_eq!(
        string_for_type(&value, root, &projector),
        Err(AdapterError::UnsupportedValue(
            "cyclic checked borrowed ABI".into()
        ))
    );
    let invalid = InterpValue::Trust {
        wrapper: TrustWrapper::Untrusted,
        value: SharedValue::new(InterpValue::String("end".into())),
    };
    assert!(
        matches!(string_for_type(&invalid, root, &projector), Err(AdapterError::TypeMismatch { expected, .. }) if expected == cyclic)
    );

    let mut store = TypeStore::new();
    let cyclic = store.intern(Type::Refined {
        base: TypeId(0),
        predicate: RefinementId(0),
    });
    let projector = PureAbiProjector::build(&store).unwrap();
    assert_eq!(
        string_for_type(&InterpValue::String("end".into()), cyclic, &projector),
        Err(AdapterError::UnsupportedValue(
            "cyclic checked borrowed ABI".into()
        ))
    );
}

#[test]
fn borrowed_projection_keeps_missing_type_errors() {
    let mut store = TypeStore::new();
    let missing = TypeId(99);
    let root = store.intern(Type::Refined {
        base: missing,
        predicate: RefinementId(0),
    });
    let projector = PureAbiProjector::build(&store).unwrap();
    let value = InterpValue::String("end".into());
    assert_eq!(
        string_for_type(&value, root, &projector),
        Err(AdapterError::MissingType(missing))
    );
    assert_eq!(
        string_for_type(&value, missing, &projector),
        Err(AdapterError::MissingType(missing))
    );
}

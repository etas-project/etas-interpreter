use super::*;
use crate::{
    intrinsic::pure::abi::{
        PureAbiProjector, input::into_builtin_for_type, output::from_builtin_for_type,
    },
    value::InterpValue,
};
use etas_builtin::BuiltinValue;
use etas_types::{EnumTypeRef, EnumVariantLayoutFact, PrimitiveType, Type};

#[test]
fn enum_abi_requires_owner_constructor_arity_and_typed_payload() {
    let mut types = etas_types::TypeInterner::new();
    let text = types.primitive(PrimitiveType::String);
    let first = types.intern(Type::Enum(EnumTypeRef {
        name: "First".into(),
    }));
    let second = types.intern(Type::Enum(EnumTypeRef {
        name: "Second".into(),
    }));
    let layout = EnumLayoutFact {
        type_params: vec![],
        variants: vec![EnumVariantLayoutFact {
            name: "Named".into(),
            fields: vec![text],
            field_names: Some(vec!["value".into()]),
        }],
    };
    let layouts = HashMap::from([(first, layout.clone()), (second, layout)]);
    let types = types.into_store();
    let projector = PureAbiProjector::build(&types, &layouts).unwrap();
    let raw = |name: &str, fields: Vec<_>| InterpValue::Variant {
        name: name.into(),
        fields: fields.into(),
    };
    let owned = |ty, name, fields| InterpValue::Nominal {
        ty,
        value: raw(name, fields).into(),
    };
    let good = owned(first, "Named", vec![InterpValue::String("checked".into())]);
    let converted = into_builtin_for_type(good.clone(), first, &projector).unwrap();
    assert_eq!(
        from_builtin_for_type(converted, first, &projector).unwrap(),
        good
    );
    assert!(
        matches!(into_builtin_for_type(owned(second, "Named", vec![InterpValue::String("checked".into())]), first, &projector), Err(AdapterError::NominalIdentity { actual, expected }) if actual == second && expected == first)
    );
    assert!(matches!(
        into_builtin_for_type(
            raw("Named", vec![InterpValue::String("checked".into())]),
            first,
            &projector
        ),
        Err(AdapterError::TypeMismatch { .. })
    ));
    assert!(matches!(
        into_builtin_for_type(owned(first, "Named", vec![]), first, &projector),
        Err(AdapterError::Arity {
            expected: 1,
            actual: 0
        })
    ));
    assert!(matches!(
        into_builtin_for_type(owned(first, "Unknown", vec![]), first, &projector),
        Err(AdapterError::UnsupportedValue(_))
    ));
    assert!(
        matches!(into_builtin_for_type(owned(first, "Named", vec![InterpValue::Bool(true)]), first, &projector), Err(AdapterError::TypeMismatch { expected, .. }) if expected == text)
    );
    assert!(
        matches!(from_builtin_for_type(BuiltinValue::Variant { name: "Named".into(), fields: vec![BuiltinValue::Bool(true)] }, first, &projector), Err(AdapterError::TypeMismatch { expected, .. }) if expected == text)
    );
    let missing = PureAbiProjector::build(&types, &HashMap::new()).unwrap();
    assert!(
        matches!(from_builtin_for_type(BuiltinValue::Variant { name: "Named".into(), fields: vec![BuiltinValue::String("checked".into())] }, first, &missing), Err(AdapterError::UnsupportedValue(reason)) if reason.contains("checked payload layout"))
    );
}

#[test]
fn enum_abi_does_not_infinitely_expand_nonuniform_recursive_definitions() {
    let checked = crate::testing::project::checked_project(
        "module app.main; enum Growing<T> { Leaf(T), Next(Growing<List<T>>), } flow main(value: Growing<i32>) -> unit { return; }",
    );
    let etas_types::ItemSignature::Flow(signature) =
        &checked.types.item_signatures[&checked.entry.unwrap()]
    else {
        panic!("entry");
    };
    let ty = signature.params[0];
    let projector =
        PureAbiProjector::build(&checked.type_store, &checked.types.enum_layouts).unwrap();
    let value = InterpValue::Nominal {
        ty,
        value: InterpValue::Variant {
            name: "Leaf".into(),
            fields: vec![InterpValue::i32(7)].into(),
        }
        .into(),
    };
    let encoded = into_builtin_for_type(value.clone(), ty, &projector).unwrap();
    assert_eq!(
        from_builtin_for_type(encoded, ty, &projector).unwrap(),
        value
    );
}

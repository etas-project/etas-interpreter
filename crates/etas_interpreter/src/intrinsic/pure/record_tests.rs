use etas_builtin::BuiltinValue;
use etas_types::{FieldType, NominalTypeRef, PrimitiveType, RecordType, Type, TypeInterner};

use super::{AdapterError, PureAbiProjector};
use crate::value::{InterpValue, RecordValue};

#[test]
fn checked_record_result_abi_moves_names_and_payloads_without_cloning_shape() {
    use super::abi::input::into_builtin_for_type;
    use super::abi::output::from_builtin_for_type;
    use crate::testing::allocation::measure;

    for count in [1000, 2000, 4000] {
        let mut interner = TypeInterner::new();
        let string = interner.primitive(PrimitiveType::String);
        let fields: Vec<_> = (0..count)
            .map(|i| FieldType {
                name: format!("field_{i:04}"),
                ty: string,
            })
            .collect();
        let ty = interner.intern(Type::Record(RecordType { fields }));
        let projector = PureAbiProjector::build(&interner.into_store()).unwrap();
        let fields: Vec<_> = (0..count)
            .map(|i| {
                (
                    format!("field_{i:04}"),
                    BuiltinValue::String(format!("{i:04}{}", "x".repeat(128))),
                )
            })
            .collect();
        let pointers: Vec<_> = fields
            .iter()
            .map(|(name, value)| {
                let BuiltinValue::String(value) = value else {
                    panic!("string payload")
                };
                (name.as_ptr(), value.as_ptr())
            })
            .collect();
        let value = BuiltinValue::Record(fields.into_iter().rev().collect());
        let (result, allocations) =
            measure(|| from_builtin_for_type(value, ty, &projector).unwrap());
        let output_cost = allocations;
        let InterpValue::Record(result) = result else {
            panic!("record result")
        };
        for ((name, value), (name_pointer, payload_pointer)) in
            result.borrow().iter().zip(pointers.iter().copied())
        {
            let InterpValue::String(value) = value else {
                panic!("string payload")
            };
            assert_eq!(name.as_ptr(), name_pointer);
            assert_eq!(value.as_ptr(), payload_pointer);
        }
        let (field, query_cost) = measure(|| result.get("field_0000").unwrap());
        drop(field);
        let (roundtrip, allocations) =
            measure(|| into_builtin_for_type(InterpValue::Record(result), ty, &projector).unwrap());
        eprintln!(
            "record ABI n={count}: output={output_cost:?}, first query={query_cost:?}, input={allocations:?}"
        );
        assert_eq!(
            output_cost.count,
            count + 2,
            "only text owners and record output storage: {output_cost:?}"
        );
        assert_eq!(
            query_cost.count, 0,
            "checked record layout was rebuilt: {query_cost:?}"
        );
        assert!(
            allocations.count <= 1,
            "temporary input field table allocated: {allocations:?}"
        );
        let BuiltinValue::Record(fields) = roundtrip else {
            panic!("builtin record")
        };
        for ((name, value), (name_pointer, payload_pointer)) in fields.iter().zip(pointers) {
            let BuiltinValue::String(value) = value else {
                panic!("string payload")
            };
            assert_eq!(name.as_ptr(), name_pointer);
            assert_eq!(value.as_ptr(), payload_pointer);
        }
    }
}

#[test]
fn checked_record_result_abi_rejects_malformed_fields_and_types() {
    use super::abi::output::from_builtin_for_type;
    let mut interner = TypeInterner::new();
    let int = interner.primitive(PrimitiveType::I32);
    let ty = interner.intern(Type::Record(RecordType {
        fields: ["a", "b"]
            .into_iter()
            .map(|name| FieldType {
                name: name.into(),
                ty: int,
            })
            .collect(),
    }));
    let projector = PureAbiProjector::build(&interner.into_store()).unwrap();
    for fields in [
        vec![("a", BuiltinValue::I32(1))],
        vec![("a", BuiltinValue::I32(1)), ("a", BuiltinValue::I32(2))],
        vec![
            ("a", BuiltinValue::I32(1)),
            ("unknown", BuiltinValue::I32(2)),
        ],
        vec![("a", BuiltinValue::I32(1)), ("b", BuiltinValue::Bool(true))],
    ] {
        let value = BuiltinValue::Record(
            fields
                .into_iter()
                .map(|(name, value)| (name.into(), value))
                .collect(),
        );
        assert!(from_builtin_for_type(value, ty, &projector).is_err());
    }
}

#[test]
fn checked_record_abi_keeps_shared_inputs_and_validates_nested_nominal_trust_fields() {
    use super::abi::{input::into_builtin_for_type, output::from_builtin_for_type};
    use crate::value::SharedValue;
    use etas_types::TrustWrapper;

    let mut interner = TypeInterner::new();
    let int = interner.primitive(PrimitiveType::I32);
    let string = interner.primitive(PrimitiveType::String);
    let trusted = interner.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: string,
    });
    let id = interner.intern(Type::Nominal(NominalTypeRef {
        name: "Id".into(),
        params: vec![],
        representation: Some(int),
    }));
    let other = interner.intern(Type::Nominal(NominalTypeRef {
        name: "Other".into(),
        params: vec![],
        representation: Some(int),
    }));
    let record = interner.intern(Type::Record(RecordType {
        fields: vec![
            FieldType {
                name: "text".into(),
                ty: trusted,
            },
            FieldType {
                name: "id".into(),
                ty: id,
            },
        ],
    }));
    let projector = PureAbiProjector::build(&interner.into_store()).unwrap();
    let input = |nominal, wrapper| {
        RecordValue::new(vec![
            (
                "id".into(),
                InterpValue::Nominal {
                    ty: nominal,
                    value: SharedValue::new(InterpValue::i32(7)),
                },
            ),
            (
                "text".into(),
                InterpValue::Trust {
                    wrapper,
                    value: SharedValue::new(InterpValue::String("retained".into())),
                },
            ),
        ])
    };
    let original = input(id, TrustWrapper::Trusted);
    let output =
        into_builtin_for_type(InterpValue::Record(original.clone()), record, &projector).unwrap();
    assert_eq!(
        output,
        BuiltinValue::Record(vec![
            ("text".into(), BuiltinValue::String("retained".into())),
            ("id".into(), BuiltinValue::I32(7)),
        ])
    );
    assert_eq!(original.borrow()[0].0, "id");
    let InterpValue::Record(mut restored) =
        from_builtin_for_type(output, record, &projector).unwrap()
    else {
        panic!("record")
    };
    assert_eq!(restored.get("id"), original.get("id"));
    assert_eq!(restored.get("text"), original.get("text"));
    let retained = restored.clone();
    restored.borrow_mut().swap(0, 1);
    assert_eq!(restored.get("text"), retained.get("text"));
    restored.borrow_mut()[0].0 = "unknown".into();
    assert!(into_builtin_for_type(InterpValue::Record(restored), record, &projector).is_err());
    assert_eq!(retained.get("id"), original.get("id"));
    assert!(
        matches!(into_builtin_for_type(InterpValue::Record(input(other, TrustWrapper::Trusted)), record, &projector), Err(AdapterError::NominalIdentity { expected, actual }) if expected == id && actual == other)
    );
    assert!(
        matches!(into_builtin_for_type(InterpValue::Record(input(id, TrustWrapper::Untrusted)), record, &projector), Err(AdapterError::TypeMismatch { expected, .. }) if expected == trusted)
    );
}

#[test]
fn checked_record_abi_reorders_fields_and_rejects_duplicate_names() {
    use super::abi::input::into_builtin_for_type;
    let mut interner = TypeInterner::new();
    let int = interner.primitive(PrimitiveType::I32);
    let ty = interner.intern(Type::Record(RecordType {
        fields: vec![
            FieldType {
                name: "a".into(),
                ty: int,
            },
            FieldType {
                name: "b".into(),
                ty: int,
            },
        ],
    }));
    let projector = PureAbiProjector::build(&interner.into_store()).unwrap();
    let value = |names: [&str; 2]| {
        InterpValue::Record(RecordValue::new(vec![
            (names[0].into(), InterpValue::i32(2)),
            (names[1].into(), InterpValue::i32(1)),
        ]))
    };
    assert_eq!(
        into_builtin_for_type(value(["b", "a"]), ty, &projector).unwrap(),
        BuiltinValue::Record(vec![
            ("a".into(), BuiltinValue::I32(1)),
            ("b".into(), BuiltinValue::I32(2))
        ])
    );
    assert!(into_builtin_for_type(value(["a", "a"]), ty, &projector).is_err());
    assert!(into_builtin_for_type(value(["a", "unknown"]), ty, &projector).is_err());
}

use super::{
    AdapterError, PureAbiProjector,
    abi::input::{into_builtin, into_builtin_for_type},
};
use crate::{
    testing::allocation::measure,
    value::{InterpValue, SharedFields},
};
use etas_builtin::BuiltinValue;
use etas_types::{
    EnumTypeRef, NominalTypeRef, PrimitiveType, TrustWrapper, Type, TypeId, TypeInterner,
};

fn types(count: usize, primitive: PrimitiveType) -> ([TypeId; 3], PureAbiProjector) {
    let mut types = TypeInterner::new();
    let value = types.primitive(primitive);
    let tuple = types.intern(Type::Tuple(vec![value; count]));
    let variant = types.intern(Type::Enum(EnumTypeRef {
        name: "Payload".into(),
    }));
    (
        [tuple, variant, variant],
        PureAbiProjector::build(&types.into_store()).unwrap(),
    )
}

fn input(kind: usize, fields: Vec<InterpValue>) -> InterpValue {
    if kind == 0 {
        InterpValue::Tuple(fields.into())
    } else {
        InterpValue::Variant {
            name: "Payload".into(),
            fields: fields.into(),
        }
    }
}

fn project(
    kind: usize,
    value: InterpValue,
    ty: TypeId,
    projector: &PureAbiProjector,
) -> Result<BuiltinValue, AdapterError> {
    if kind == 2 {
        into_builtin(value)
    } else {
        into_builtin_for_type(value, ty, projector)
    }
}

#[test]
fn shared_tuple_and_variant_abi_inputs_allocate_only_final_sized_output() {
    let mut staged = Vec::new();
    for count in [1000, 2000, 4000] {
        let (types, projector) = types(count, PrimitiveType::I32);
        for (kind, ty) in types.into_iter().enumerate() {
            let original = input(
                kind,
                (0..count).map(|i| InterpValue::i32(i as i32)).collect(),
            );
            let (output, cost) =
                measure(|| project(kind, original.clone(), ty, &projector).unwrap());
            let BuiltinValue::Variant { name, fields } = output else {
                panic!("variant ABI output")
            };
            let required = count * std::mem::size_of::<BuiltinValue>() + name.len();
            eprintln!(
                "shared field ABI kind={kind} n={count}: {cost:?}; required={required}; output_capacity={}",
                fields.capacity()
            );
            if cost.bytes != required || cost.count != 2 || fields.capacity() != count {
                staged.push((kind, count, cost, fields.capacity()));
            }
            assert_eq!(name, if kind == 0 { "Tuple" } else { "Payload" });
            assert_eq!(fields.len(), count);
            for (index, value) in fields.iter().enumerate() {
                assert_eq!(*value, BuiltinValue::I32(index as i32));
            }
            let retained = match &original {
                InterpValue::Tuple(fields) | InterpValue::Variant { fields, .. } => fields,
                _ => panic!("original fields"),
            };
            assert_eq!(retained.len(), count);
            assert_eq!(retained[count - 1], InterpValue::i32(count as i32 - 1));
        }
    }
    assert!(
        staged.is_empty(),
        "runtime field staging buffer: {staged:?}"
    );
}

#[test]
fn unique_tuple_and_variant_abi_inputs_move_text_payloads() {
    for count in [1000, 2000, 4000] {
        let (types, projector) = types(count, PrimitiveType::String);
        for (kind, ty) in types.into_iter().enumerate() {
            let texts: Vec<_> = (0..count)
                .map(|i| format!("{i:04}{}", "x".repeat(1024)))
                .collect();
            let pointers: Vec<_> = texts.iter().map(|text| text.as_ptr()).collect();
            let original = input(
                kind,
                texts
                    .into_iter()
                    .map(|text| InterpValue::String(text.into()))
                    .collect(),
            );
            let (output, cost) = measure(|| project(kind, original, ty, &projector).unwrap());
            assert_eq!(
                cost.count,
                usize::from(kind == 0),
                "only the Tuple tag may allocate: {cost:?}"
            );
            let BuiltinValue::Variant { fields, .. } = output else {
                panic!("variant ABI output")
            };
            for (value, pointer) in fields.iter().zip(pointers) {
                let BuiltinValue::String(text) = value else {
                    panic!("text output")
                };
                assert_eq!(text.as_ptr(), pointer);
            }
            eprintln!("unique field ABI kind={kind} n={count}: {cost:?}");
        }
    }
}

#[test]
fn tuple_abi_projection_preserves_checked_identity_and_releases_failed_output() {
    let mut types = TypeInterner::new();
    let text = types.primitive(PrimitiveType::String);
    let nominal = types.intern(Type::Nominal(NominalTypeRef {
        name: "Id".into(),
        params: vec![],
        representation: Some(text),
    }));
    let other = types.intern(Type::Nominal(NominalTypeRef {
        name: "OtherId".into(),
        params: vec![],
        representation: Some(text),
    }));
    let trusted = types.intern(Type::Trust {
        wrapper: TrustWrapper::Trusted,
        inner: text,
    });
    let tuple = types.intern(Type::Tuple(vec![nominal, trusted]));
    let projector = PureAbiProjector::build(&types.into_store()).unwrap();
    let value = |nominal, wrapper| {
        InterpValue::Tuple(SharedFields::new(vec![
            InterpValue::Nominal {
                ty: nominal,
                value: InterpValue::String("id".repeat(512).into()).into(),
            },
            InterpValue::Trust {
                wrapper,
                value: InterpValue::String("text".repeat(512).into()).into(),
            },
        ]))
    };
    let original = value(nominal, TrustWrapper::Trusted);
    let output = into_builtin_for_type(original.clone(), tuple, &projector).unwrap();
    assert_eq!(
        output,
        BuiltinValue::Variant {
            name: "Tuple".into(),
            fields: vec![
                BuiltinValue::String("id".repeat(512)),
                BuiltinValue::String("text".repeat(512))
            ]
        }
    );
    for (bad, identity) in [
        (value(other, TrustWrapper::Trusted), true),
        (value(nominal, TrustWrapper::Untrusted), false),
    ] {
        let retained = bad.clone();
        let (_, cost) = measure(|| {
            let error = into_builtin_for_type(bad.clone(), tuple, &projector).unwrap_err();
            if identity {
                assert!(
                    matches!(error, AdapterError::NominalIdentity { expected, actual } if expected == nominal && actual == other)
                );
            } else {
                assert!(
                    matches!(error, AdapterError::TypeMismatch { expected, .. } if expected == trusted)
                );
            }
        });
        assert_eq!(
            cost.bytes, cost.released_bytes,
            "partial output leaked: {cost:?}"
        );
        assert_eq!(bad, retained);
    }
    for malformed in [
        InterpValue::Unit,
        InterpValue::Tuple(vec![InterpValue::i32(1)].into()),
    ] {
        assert!(
            matches!(into_builtin_for_type(malformed, tuple, &projector), Err(AdapterError::TypeMismatch { expected, .. }) if expected == tuple)
        );
    }
}

#[test]
fn enum_payload_projection_rejects_unchecked_nominal_and_releases_partial_output() {
    let (types, projector) = types(2, PrimitiveType::String);
    for (kind, ty) in types.into_iter().enumerate().skip(1) {
        let original = input(
            kind,
            vec![
                InterpValue::String("prefix".repeat(1024).into()),
                InterpValue::Nominal {
                    ty: TypeId(999),
                    value: InterpValue::i32(7).into(),
                },
            ],
        );
        let retained = original.clone();
        let (_, cost) = measure(|| {
            assert!(matches!(
                project(kind, original.clone(), ty, &projector),
                Err(AdapterError::UnsupportedValue(_))
            ))
        });
        assert_eq!(
            cost.bytes, cost.released_bytes,
            "partial enum output leaked: {cost:?}"
        );
        assert_eq!(original, retained);
    }
}

#[test]
fn malformed_abi_diagnostics_never_render_secret_payloads() {
    let marker = "must-not-appear-in-abi-errors";
    let (types, projector) = types(1, PrimitiveType::String);
    let mut leaks = Vec::new();
    for (kind, ty) in types.into_iter().enumerate().skip(1) {
        let secret = InterpValue::Trust {
            wrapper: TrustWrapper::Secret,
            value: InterpValue::String(marker.into()).into(),
        };
        let error = project(kind, input(kind, vec![secret]), ty, &projector).unwrap_err();
        if format!("{error:?}").contains(marker) {
            leaks.push(if kind == 1 {
                "enum input"
            } else {
                "enum payload input"
            });
        }
    }
    let mut interner = TypeInterner::new();
    let boolean = interner.primitive(PrimitiveType::Bool);
    let tuple = interner.intern(Type::Tuple(vec![boolean]));
    let projector = PureAbiProjector::build(&interner.into_store()).unwrap();
    for (expected, value) in [
        (boolean, BuiltinValue::String(marker.into())),
        (
            boolean,
            BuiltinValue::Record(vec![(marker.into(), BuiltinValue::String(marker.into()))]),
        ),
        (
            tuple,
            BuiltinValue::Variant {
                name: marker.into(),
                fields: vec![BuiltinValue::String(marker.into())],
            },
        ),
    ] {
        let error =
            super::abi::output::from_builtin_for_type(value, expected, &projector).unwrap_err();
        assert!(
            matches!(&error, AdapterError::TypeMismatch { expected: actual, .. } if *actual == expected)
        );
        if format!("{error:?}").contains(marker) {
            leaks.push("builtin result");
        }
    }
    assert!(leaks.is_empty(), "ABI errors rendered data in {leaks:?}");
}

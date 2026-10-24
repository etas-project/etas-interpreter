use super::super::*;
use crate::testing::allocation::measure;
use etas_types::{FieldType, RecordType, TypeInterner};

#[test]
fn checked_host_record_decode_moves_owned_names_and_payloads() {
    for count in [1000, 2000, 4000] {
        let mut interner = TypeInterner::new();
        let string = interner.primitive(PrimitiveType::String);
        let ty = interner.intern(Type::Record(RecordType {
            fields: (0..count)
                .map(|i| FieldType {
                    name: format!("field_{i:04}"),
                    ty: string,
                })
                .collect(),
        }));
        let store = interner.into_store();
        let fields: Vec<_> = (0..count)
            .map(|i| (format!("field_{i:04}"), HostValue::String("x".repeat(128))))
            .collect();
        let pointers: Vec<_> = fields
            .iter()
            .map(|(key, value)| {
                let HostValue::String(value) = value else {
                    panic!("string")
                };
                (key.as_ptr(), value.as_ptr())
            })
            .collect();
        let value = HostValue::Record(fields.into_iter().rev().collect());
        let (decoded, allocations) =
            measure(|| host_to_typed_interp_value(value, ty, &store).unwrap());
        assert_eq!(
            allocations.count,
            count + 4,
            "one shared text owner per field, field index, known-name set, vector, record backing: {count}"
        );
        let InterpValue::Record(decoded) = decoded else {
            panic!("record")
        };
        for ((key, value), (key_pointer, value_pointer)) in decoded.borrow().iter().zip(pointers) {
            let InterpValue::String(value) = value else {
                panic!("string")
            };
            assert_eq!(key.as_ptr(), key_pointer);
            assert_eq!(value.as_ptr(), value_pointer);
        }
    }
}

#[test]
fn checked_host_record_decode_rejects_bad_field_sets_and_payloads() {
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
    let store = interner.into_store();
    for (fields, expected_error) in [
        (vec![("a", HostValue::Int(1))], "missing field `b`"),
        (
            vec![("a", HostValue::Int(1)), ("a", HostValue::Int(2))],
            "duplicate field `a`",
        ),
        (
            vec![("a", HostValue::Int(1)), ("unknown", HostValue::Int(2))],
            "unknown field `unknown`",
        ),
        (
            vec![("a", HostValue::Int(1)), ("b", HostValue::Bool(true))],
            "record field `b`",
        ),
    ] {
        let value = HostValue::Record(
            fields
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        );
        let error = host_to_typed_interp_value(value, ty, &store).unwrap_err();
        assert!(error.contains(expected_error), "{error}");
    }
}

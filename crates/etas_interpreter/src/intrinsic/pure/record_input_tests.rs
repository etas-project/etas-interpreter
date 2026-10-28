use super::{AdapterError, PureAbiProjector, abi::input::into_builtin_for_type};
use crate::{
    testing::allocation::measure,
    value::{InterpValue, RecordValue},
};
use etas_builtin::BuiltinValue;
use etas_types::{FieldType, PrimitiveType, RecordType, Type, TypeId, TypeInterner};

fn record_type(count: usize, primitive: PrimitiveType) -> (TypeId, TypeId, PureAbiProjector) {
    let mut interner = TypeInterner::new();
    let inner = interner.primitive(primitive);
    let ty = interner.intern(Type::Record(RecordType {
        fields: (0..count)
            .map(|i| FieldType {
                name: format!("field_{i:04}"),
                ty: inner,
            })
            .collect(),
    }));
    (
        ty,
        inner,
        PureAbiProjector::build(&interner.into_store()).unwrap(),
    )
}

#[test]
fn shared_record_input_projects_into_final_sized_abi_storage() {
    let mut staged = Vec::new();
    for count in [1000, 2000, 4000] {
        let (ty, _, projector) = record_type(count, PrimitiveType::I32);
        for reversed in [false, true] {
            let mut fields: Vec<_> = (0..count)
                .map(|i| (format!("field_{i:04}"), InterpValue::i32(i as i32)))
                .collect();
            if reversed {
                fields.reverse();
            }
            let input = RecordValue::new(fields);
            let name_bytes: usize = input.borrow().iter().map(|(name, _)| name.len()).sum();
            let (output, cost) = measure(|| {
                into_builtin_for_type(InterpValue::Record(input.clone()), ty, &projector).unwrap()
            });
            let BuiltinValue::Record(output) = output else {
                panic!("record output")
            };
            let required = count * std::mem::size_of::<(String, BuiltinValue)>() + name_bytes;
            eprintln!(
                "shared record input n={count} reversed={reversed}: {cost:?}; required={required}; output_capacity={}",
                output.capacity()
            );
            if cost.bytes > required || cost.count != count + 1 || output.capacity() != count {
                staged.push((count, reversed, cost, output.capacity()));
            }
            assert_eq!(output.len(), count);
            for (index, (name, value)) in output.iter().enumerate() {
                assert_eq!(name, &format!("field_{index:04}"));
                assert_eq!(value, &BuiltinValue::I32(index as i32));
            }
            assert_eq!(input.borrow().len(), count);
            assert_eq!(
                input.borrow()[0].1,
                InterpValue::i32(if reversed { count as i32 - 1 } else { 0 })
            );
        }
    }
    assert!(staged.is_empty(), "staged runtime record table: {staged:?}");
}

#[test]
fn unique_record_input_moves_field_names_and_text_payloads() {
    for count in [1000, 2000, 4000] {
        let (ty, _, projector) = record_type(count, PrimitiveType::String);
        let fields: Vec<_> = (0..count)
            .map(|i| {
                (
                    format!("field_{i:04}"),
                    InterpValue::String(format!("{i:04}{}", "x".repeat(1024)).into()),
                )
            })
            .collect();
        let pointers: Vec<_> = fields
            .iter()
            .map(|(name, value)| {
                let InterpValue::String(text) = value else {
                    panic!("text input")
                };
                (name.as_ptr(), text.as_ptr())
            })
            .collect();
        let input = RecordValue::new(fields.into_iter().rev().collect());
        let (output, cost) =
            measure(|| into_builtin_for_type(InterpValue::Record(input), ty, &projector).unwrap());
        assert!(cost.count <= 1, "copied unique names/payload: {cost:?}");
        let BuiltinValue::Record(output) = output else {
            panic!("record output")
        };
        for ((name, value), (name_pointer, text_pointer)) in output.iter().zip(pointers) {
            let BuiltinValue::String(text) = value else {
                panic!("text output")
            };
            assert_eq!(name.as_ptr(), name_pointer);
            assert_eq!(text.as_ptr(), text_pointer);
        }
        eprintln!("unique record input n={count}: {cost:?}");
    }
}

#[test]
fn failed_shared_record_input_releases_partial_output_and_preserves_aliases() {
    let (ty, inner, projector) = record_type(3, PrimitiveType::String);
    let input = RecordValue::new(vec![
        (
            "field_0000".into(),
            InterpValue::String("x".repeat(1024).into()),
        ),
        ("field_0001".into(), InterpValue::Bool(true)),
        (
            "field_0002".into(),
            InterpValue::String("z".repeat(1024).into()),
        ),
    ]);
    let retained = input.clone();
    let (_, cost) = measure(|| {
        assert!(
            matches!(into_builtin_for_type(InterpValue::Record(input.clone()), ty, &projector), Err(AdapterError::TypeMismatch { expected, .. }) if expected == inner)
        );
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "partial output leak: {cost:?}"
    );
    assert_eq!(input, retained);
    for names in [
        vec!["field_0000"],
        vec!["field_0000", "field_0000", "field_0002"],
        vec!["field_0000", "unknown", "field_0002"],
    ] {
        let bad = RecordValue::new(
            names
                .into_iter()
                .map(|name| (name.into(), InterpValue::String("value".into())))
                .collect(),
        );
        let (_, cost) = measure(|| {
            assert!(
                matches!(into_builtin_for_type(InterpValue::Record(bad.clone()), ty, &projector), Err(AdapterError::TypeMismatch { expected, .. }) if expected == ty)
            );
        });
        assert_eq!(cost.bytes, cost.released_bytes);
        assert!(matches!(bad.borrow()[0].1, InterpValue::String(_)));
    }
}

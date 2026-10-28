use super::{AdapterError, PureAbiProjector, abi::input::into_builtin_for_type};
use crate::{testing::allocation::measure, value::InterpValue};
use etas_builtin::BuiltinValue;
use etas_types::{PrimitiveType, Type, TypeInterner};

#[test]
fn shared_sequence_abi_projection_does_not_stage_a_runtime_payload_vector() {
    let mut staged = Vec::new();
    for count in [1000, 2000, 4000] {
        let mut interner = TypeInterner::new();
        let int = interner.primitive(PrimitiveType::I32);
        let types = [
            Type::Array(int),
            Type::List(int),
            Type::Slice(int),
            Type::Set(int),
            Type::Map {
                key: int,
                value: int,
            },
        ];
        let types = types.map(|ty| interner.intern(ty));
        let projector =
            PureAbiProjector::build(&interner.into_store(), &Default::default()).unwrap();
        for (kind, ty) in types.into_iter().enumerate() {
            let values = || {
                (0..count)
                    .map(|i| InterpValue::i32(i as i32))
                    .collect::<Vec<_>>()
            };
            let original = match kind {
                0 => InterpValue::Array(values().into()),
                1 => InterpValue::List(values().into()),
                2 => InterpValue::Slice(values().into()),
                3 => InterpValue::Set(values().into()),
                _ => InterpValue::Map(
                    (0..count)
                        .map(|i| (InterpValue::i32(i as i32), InterpValue::i32(i as i32)))
                        .collect::<Vec<_>>()
                        .into(),
                ),
            };
            let (output, cost) =
                measure(|| into_builtin_for_type(original.clone(), ty, &projector).unwrap());
            let element_size = if kind == 4 {
                std::mem::size_of::<(BuiltinValue, BuiltinValue)>()
            } else {
                std::mem::size_of::<BuiltinValue>()
            };
            eprintln!(
                "shared pure ABI kind={kind} n={count}: {cost:?}; final element={element_size}B"
            );
            if cost.bytes > count * element_size || cost.count > 1 {
                staged.push((kind, count, cost));
            }
            let output = match output {
                BuiltinValue::Array(values)
                | BuiltinValue::List(values)
                | BuiltinValue::Slice(values)
                | BuiltinValue::Set(values) => values,
                BuiltinValue::Map(entries) => {
                    assert_eq!(entries.len(), count);
                    assert_eq!(entries[0], (BuiltinValue::I32(0), BuiltinValue::I32(0)));
                    assert!(into_builtin_for_type(original, ty, &projector).is_ok());
                    continue;
                }
                _ => panic!("sequence result"),
            };
            assert_eq!(output.len(), count);
            assert_eq!(output[0], BuiltinValue::I32(0));
            assert_eq!(output[count - 1], BuiltinValue::I32(count as i32 - 1));
            assert!(into_builtin_for_type(original, ty, &projector).is_ok());
        }
    }
    assert!(staged.is_empty(), "runtime staging allocations: {staged:?}");
}

fn sequence(kind: usize, values: Vec<InterpValue>) -> InterpValue {
    match kind {
        0 => InterpValue::Array(values.into()),
        1 => InterpValue::List(values.into()),
        2 => InterpValue::Slice(values.into()),
        3 => InterpValue::Set(values.into()),
        4 => InterpValue::Map(
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| (InterpValue::i32(index as i32), value))
                .collect::<Vec<_>>()
                .into(),
        ),
        _ => panic!("unknown sequence kind"),
    }
}

fn sequence_types(
    interner: &mut TypeInterner,
    value: etas_types::TypeId,
) -> [etas_types::TypeId; 5] {
    let key = interner.primitive(PrimitiveType::I32);
    [
        Type::Array(value),
        Type::List(value),
        Type::Slice(value),
        Type::Set(value),
        Type::Map { key, value },
    ]
    .map(|ty| interner.intern(ty))
}

#[test]
fn unique_sequence_abi_projection_moves_text_payloads_without_copying() {
    for count in [1000, 2000, 4000] {
        let mut interner = TypeInterner::new();
        let text = interner.primitive(PrimitiveType::String);
        let types = sequence_types(&mut interner, text);
        let projector =
            PureAbiProjector::build(&interner.into_store(), &Default::default()).unwrap();
        for (kind, ty) in types.into_iter().enumerate() {
            let texts = (0..count)
                .map(|index| format!("{index:04}{}", "x".repeat(1024)))
                .collect::<Vec<_>>();
            let pointers = texts.iter().map(|text| text.as_ptr()).collect::<Vec<_>>();
            let input = sequence(
                kind,
                texts
                    .into_iter()
                    .map(|text| InterpValue::String(text.into()))
                    .collect(),
            );
            let (output, cost) = measure(|| into_builtin_for_type(input, ty, &projector).unwrap());
            assert!(
                cost.count <= 1,
                "copied unique payloads: kind={kind} {cost:?}"
            );
            eprintln!("unique pure ABI kind={kind} n={count}: {cost:?}");
            let output = match output {
                BuiltinValue::Array(values)
                | BuiltinValue::List(values)
                | BuiltinValue::Slice(values)
                | BuiltinValue::Set(values) => values,
                BuiltinValue::Map(entries) => entries.into_iter().map(|(_, value)| value).collect(),
                _ => panic!("sequence result"),
            };
            for (value, pointer) in output.iter().zip(pointers) {
                let BuiltinValue::String(value) = value else {
                    panic!("string result")
                };
                assert_eq!(value.as_ptr(), pointer);
            }
        }
    }
}

#[test]
fn failed_sequence_abi_projection_keeps_aliases_and_releases_partial_output() {
    let mut interner = TypeInterner::new();
    let text = interner.primitive(PrimitiveType::String);
    let types = sequence_types(&mut interner, text);
    let projector = PureAbiProjector::build(&interner.into_store(), &Default::default()).unwrap();
    for (kind, ty) in types.into_iter().enumerate() {
        let input = sequence(
            kind,
            vec![
                InterpValue::String("x".repeat(1024).into()),
                InterpValue::Bool(true),
            ],
        );
        let retained = input.clone();
        let (_, cost) = measure(|| {
            assert!(
                matches!(into_builtin_for_type(input.clone(), ty, &projector), Err(AdapterError::TypeMismatch { expected, .. }) if expected == text)
            );
        });
        assert_eq!(
            cost.bytes, cost.released_bytes,
            "leaked partial ABI output: kind={kind} {cost:?}"
        );
        assert_eq!(input, retained);
        drop(retained);
        assert!(
            matches!(into_builtin_for_type(input, ty, &projector), Err(AdapterError::TypeMismatch { expected, .. }) if expected == text)
        );
    }
}

#[test]
fn slice_abi_projection_only_materializes_the_selected_window() {
    use crate::value::{ArrayValue, SliceValue};
    let mut interner = TypeInterner::new();
    let int = interner.primitive(PrimitiveType::I32);
    let ty = interner.intern(Type::Slice(int));
    let projector = PureAbiProjector::build(&interner.into_store(), &Default::default()).unwrap();
    let mut values = vec![InterpValue::Bool(false); 4000];
    values[2000] = InterpValue::i32(4);
    values[2001] = InterpValue::i32(5);
    let array = ArrayValue::new(values);
    let slice = SliceValue::from_array(array.clone(), 2000..2002).unwrap();
    let (output, cost) =
        measure(|| into_builtin_for_type(InterpValue::Slice(slice), ty, &projector).unwrap());
    assert_eq!(
        output,
        BuiltinValue::Slice(vec![BuiltinValue::I32(4), BuiltinValue::I32(5)])
    );
    assert_eq!(cost.count, 1);
    assert_eq!(cost.bytes, 2 * std::mem::size_of::<BuiltinValue>());
    assert_eq!(array.borrow().len(), 4000);
    assert_eq!(array.borrow()[0], InterpValue::Bool(false));
}

use super::*;
use crate::testing::allocation::measure;
use etas_host::HostJsonValue as Json;

#[test]
fn structured_model_json_does_not_clone_an_intermediate_tree() {
    let mut store = TypeStore::new();
    let string = store.intern(Type::Primitive(PrimitiveType::String));
    let array = store.intern(Type::Array(string));
    for count in [1000, 2000, 4000] {
        let input = etas_host::HostJsonValue::Array(
            (0..count)
                .map(|_| etas_host::HostJsonValue::String("x".repeat(1024)))
                .collect(),
        );
        let (output, cost) =
            measure(|| json_host_value_to_typed_interp_value(&input, array, &store).unwrap());
        eprintln!("structured model JSON n={count}: {cost:?}");
        assert!(
            cost.bytes < count * (1024 + 512),
            "intermediate JSON tree materialized: {cost:?}"
        );
        let InterpValue::Array(values) = output else {
            panic!("array")
        };
        assert_eq!(values.borrow().len(), count);
        assert!(
            values
                .borrow()
                .iter()
                .all(|value| matches!(value, InterpValue::String(text) if text.len() == 1024))
        );
    }
}

// Reference the former tree conversion only in tests, to check the new borrowed
// view's number, key ordering, duplicate handling, and whole-input validity rules.
fn reference_tree(value: &Json) -> Option<serde_json::Value> {
    Some(match value {
        Json::Null => serde_json::Value::Null,
        Json::Bool(value) => serde_json::Value::Bool(*value),
        Json::Number(value) => serde_json::Value::Number(serde_json::Number::from_f64(*value)?),
        Json::String(value) => serde_json::Value::String(value.clone()),
        Json::Array(values) => {
            serde_json::Value::Array(values.iter().map(reference_tree).collect::<Option<_>>()?)
        }
        Json::Object(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(name, value)| Some((name.clone(), reference_tree(value)?)))
                .collect::<Option<_>>()?,
        ),
    })
}

#[test]
fn borrowed_model_json_matches_the_previous_typed_boundary() {
    use etas_types::{
        EnumTypeRef, FieldType, NamedTypeRef, NominalTypeRef, RecordType, TrustWrapper,
        TypeConstructorId,
    };
    let mut store = TypeStore::new();
    let mut types = [
        PrimitiveType::Unit,
        PrimitiveType::Bool,
        PrimitiveType::String,
        PrimitiveType::Bytes,
        PrimitiveType::Char,
        PrimitiveType::I8,
        PrimitiveType::U8,
        PrimitiveType::I64,
        PrimitiveType::U64,
        PrimitiveType::I128,
        PrimitiveType::U128,
        PrimitiveType::F32,
        PrimitiveType::F64,
    ]
    .map(|ty| store.intern(Type::Primitive(ty)))
    .to_vec();
    let string = store.intern(Type::Primitive(PrimitiveType::String));
    let boolean = store.intern(Type::Primitive(PrimitiveType::Bool));
    let array = store.intern(Type::Array(string));
    let record = store.intern(Type::Record(RecordType {
        fields: vec![
            FieldType {
                name: "b".into(),
                ty: array,
            },
            FieldType {
                name: "a".into(),
                ty: string,
            },
        ],
    }));
    let parameter = store.intern(Type::Named(NamedTypeRef { name: "T".into() }));
    let generic = store.intern(Type::Nominal(NominalTypeRef {
        name: "Payload".into(),
        params: vec!["T".into()],
        representation: Some(parameter),
    }));
    let applied = store.intern(Type::Applied {
        constructor: TypeConstructorId(generic.0),
        args: vec![record],
    });
    types.extend([array, record, applied]);
    types.extend(
        [
            Type::List(string),
            Type::Slice(string),
            Type::Set(string),
            Type::Option(string),
            Type::Result {
                ok: string,
                err: boolean,
            },
            Type::Tuple(vec![string, boolean]),
            Type::Map {
                key: string,
                value: string,
            },
            Type::Record(RecordType { fields: vec![] }),
            Type::Enum(EnumTypeRef {
                name: "Choice".into(),
            }),
            Type::Trust {
                wrapper: TrustWrapper::Trusted,
                inner: string,
            },
            Type::Trust {
                wrapper: TrustWrapper::Untrusted,
                inner: string,
            },
        ]
        .map(|ty| store.intern(ty)),
    );
    let inputs = vec![
        Json::Null,
        Json::Bool(true),
        Json::String("".into()),
        Json::String("é".into()),
        Json::String("multi".into()),
        Json::Array(vec![
            Json::String("first".into()),
            Json::String("second".into()),
        ]),
        Json::Array(vec![Json::String("a".into()), Json::Bool(false)]),
        Json::Object(vec![
            ("a".into(), Json::String("first".into())),
            ("b".into(), Json::Array(vec![])),
            ("a".into(), Json::String("last".into())),
        ]),
        Json::Object(vec![
            ("z".into(), Json::String("z".into())),
            ("a".into(), Json::String("a".into())),
        ]),
        Json::Array(vec![Json::Array(vec![
            Json::String("key".into()),
            Json::String("value".into()),
        ])]),
        Json::Array(vec![Json::Object(vec![
            ("key".into(), Json::String("key".into())),
            ("value".into(), Json::String("value".into())),
        ])]),
        Json::Object(vec![("Empty".into(), Json::Null)]),
        Json::Object(vec![("Empty".into(), Json::Array(vec![]))]),
        Json::Object(vec![("Value".into(), Json::Array(vec![Json::Bool(false)]))]),
        Json::Object(vec![
            ("Err".into(), Json::Bool(false)),
            ("Ok".into(), Json::String("ok".into())),
        ]),
        Json::Object(vec![
            ("Ok".into(), Json::Bool(false)),
            ("Err".into(), Json::Bool(false)),
        ]),
        Json::Object(vec![("unused".into(), Json::Number(f64::NAN))]),
        Json::Object(vec![
            ("a".into(), Json::Number(f64::INFINITY)),
            ("a".into(), Json::String("last".into())),
        ]),
    ];
    let inputs = inputs.into_iter().chain(
        [
            0.0,
            -0.0,
            1.0,
            1.5,
            -128.0,
            256.0,
            f64::MAX,
            f64::MIN_POSITIVE,
            f64::NAN,
            f64::NEG_INFINITY,
        ]
        .map(Json::Number),
    );
    for input in inputs {
        let tree = reference_tree(&input);
        for ty in &types {
            let expected = tree
                .as_ref()
                .and_then(|value| json_to_typed_interp_value(value, *ty, &store));
            let actual = json_host_value_to_typed_interp_value(&input, *ty, &store);
            assert_eq!(actual, expected, "{ty:?} {input:?}");
        }
    }
}

#[test]
fn wide_model_json_objects_only_allocate_a_shallow_reference_index() {
    for count in [1000, 2000, 4000] {
        let input = Json::Object(
            (0..count)
                .rev()
                .map(|i| (format!("field_{i}"), Json::String("x".repeat(1024))))
                .collect(),
        );
        let mut store = TypeStore::new();
        let empty = store.intern(Type::Record(etas_types::RecordType { fields: vec![] }));
        let (output, cost) =
            measure(|| json_host_value_to_typed_interp_value(&input, empty, &store).unwrap());
        assert!(matches!(output, InterpValue::Record(_)));
        eprintln!("model JSON object view n={count}: {cost:?}");
        assert!(
            cost.bytes < count * 64,
            "unused child graph copied: {cost:?}"
        );
        assert!(cost.count < 8, "per-field allocation: {cost:?}");
    }
}

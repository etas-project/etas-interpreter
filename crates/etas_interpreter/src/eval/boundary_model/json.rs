use super::*;
mod view;
use view::{JsonRef, valid_host_json};

pub(super) fn from_serde(
    value: &serde_json::Value,
    expected: TypeId,
    store: &TypeStore,
) -> Option<InterpValue> {
    decode(
        JsonRef::Serde(value),
        expected,
        store,
        &std::collections::HashMap::new(),
    )
}

pub(super) fn from_host(
    value: &etas_host::HostJsonValue,
    expected: TypeId,
    store: &TypeStore,
) -> Option<InterpValue> {
    if !valid_host_json(value) {
        return None;
    }
    decode(
        JsonRef::Host(value),
        expected,
        store,
        &std::collections::HashMap::new(),
    )
}

fn decode(
    value: JsonRef<'_>,
    expected: TypeId,
    store: &TypeStore,
    substitutions: &std::collections::HashMap<String, TypeId>,
) -> Option<InterpValue> {
    if let Some(Type::Named(named)) = store.get(expected)
        && let Some(expected) = substitutions.get(&named.name).copied()
    {
        return decode(value, expected, store, substitutions);
    }
    match store.get(expected)? {
        Type::Primitive(primitive) => json_to_primitive(value, *primitive),
        Type::Array(elem) => json_array_to_values(value, *elem, store, substitutions)
            .map(ArrayValue::new)
            .map(InterpValue::Array),
        Type::List(elem) => json_array_to_values(value, *elem, store, substitutions)
            .map(|values| InterpValue::List(values.into())),
        Type::Slice(elem) => json_array_to_values(value, *elem, store, substitutions)
            .map(SliceValue::new)
            .map(InterpValue::Slice),
        Type::Set(elem) => json_array_to_values(value, *elem, store, substitutions)
            .map(|values| InterpValue::Set(values.into())),
        Type::Map { key, value: elem } => json_to_map(value, *key, *elem, store, substitutions),
        Type::Record(record) => {
            let object = value.as_object()?;
            record
                .fields
                .iter()
                .map(|field| {
                    let field_value = object.get(&field.name)?;
                    Some((
                        field.name.clone(),
                        decode(field_value, field.ty, store, substitutions)?,
                    ))
                })
                .collect::<Option<Vec<_>>>()
                .map(|fields| InterpValue::Record(fields.into()))
        }
        Type::Nominal(nominal) => {
            let representation = nominal.representation?;
            decode(value, representation, store, substitutions).map(|value| InterpValue::Nominal {
                ty: expected,
                value: crate::value::SharedValue::new(value),
            })
        }
        Type::Applied { constructor, args } => {
            let Type::Nominal(nominal) = store.get(TypeId(constructor.0))? else {
                return None;
            };
            if nominal.params.len() != args.len() {
                return None;
            }
            let mut applied_substitutions = substitutions.clone();
            applied_substitutions.extend(nominal.params.iter().cloned().zip(args.iter().copied()));
            decode(
                value,
                nominal.representation?,
                store,
                &applied_substitutions,
            )
            .map(|value| InterpValue::Nominal {
                ty: expected,
                value: crate::value::SharedValue::new(value),
            })
        }
        Type::Tuple(types) => {
            let values = value.as_array()?;
            if values.len() != types.len() {
                return None;
            }
            values
                .iter()
                .zip(types.iter())
                .map(|(value, ty)| decode(value, *ty, store, substitutions))
                .collect::<Option<Vec<_>>>()
                .map(|values| InterpValue::Tuple(values.into()))
        }
        Type::Option(inner) => {
            if value.is_null() {
                Some(InterpValue::OptionNone)
            } else {
                decode(value, *inner, store, substitutions)
                    .map(crate::value::SharedValue::new)
                    .map(InterpValue::OptionSome)
            }
        }
        Type::Result { ok, err } => json_to_result(value, *ok, *err, store, substitutions),
        Type::Enum(_) => json_to_enum(value),
        Type::Trust { wrapper, inner }
            if matches!(wrapper, etas_types::TrustWrapper::Untrusted) =>
        {
            decode(value, *inner, store, substitutions).map(|value| InterpValue::Trust {
                wrapper: *wrapper,
                value: crate::value::SharedValue::new(value),
            })
        }
        Type::Trust { .. } => None,
        Type::Schema(inner) | Type::Message(inner) => decode(value, *inner, store, substitutions),
        _ => None,
    }
}

fn json_to_primitive(value: JsonRef<'_>, primitive: PrimitiveType) -> Option<InterpValue> {
    match primitive {
        PrimitiveType::Bool => value.as_bool().map(InterpValue::Bool),
        PrimitiveType::String => value
            .as_str()
            .map(|value| InterpValue::String(value.to_owned().into())),
        PrimitiveType::Char => value
            .as_str()
            .and_then(|value| {
                let mut chars = value.chars();
                let ch = chars.next()?;
                chars.next().is_none().then_some(ch)
            })
            .map(|ch| InterpValue::String(ch.to_string().into())),
        PrimitiveType::Unit => value.is_null().then_some(InterpValue::Unit),
        PrimitiveType::Bytes => value
            .as_str()
            .map(|value| InterpValue::Bytes(value.as_bytes().to_vec().into())),
        primitive @ (PrimitiveType::I8
        | PrimitiveType::I16
        | PrimitiveType::I32
        | PrimitiveType::I64
        | PrimitiveType::I128
        | PrimitiveType::ISize) => signed_integer(value, primitive),
        primitive @ (PrimitiveType::U8
        | PrimitiveType::U16
        | PrimitiveType::U32
        | PrimitiveType::U64
        | PrimitiveType::U128
        | PrimitiveType::USize) => unsigned_integer(value, primitive),
        primitive @ (PrimitiveType::F32 | PrimitiveType::F64) => value
            .as_f64()
            .and_then(|value| crate::value::NumericValue::from_float(value, primitive))
            .map(InterpValue::Number),
        PrimitiveType::Never => None,
    }
}

fn signed_integer(value: JsonRef<'_>, primitive: PrimitiveType) -> Option<InterpValue> {
    let value = value.as_i64()? as i128;
    crate::value::NumericValue::from_signed(value, primitive).map(InterpValue::Number)
}

fn unsigned_integer(value: JsonRef<'_>, primitive: PrimitiveType) -> Option<InterpValue> {
    let value = value.as_u64()? as u128;
    crate::value::NumericValue::from_unsigned(value, primitive).map(InterpValue::Number)
}

fn json_array_to_values(
    value: JsonRef<'_>,
    elem: TypeId,
    store: &TypeStore,
    substitutions: &std::collections::HashMap<String, TypeId>,
) -> Option<Vec<InterpValue>> {
    value
        .as_array()?
        .iter()
        .map(|value| decode(value, elem, store, substitutions))
        .collect()
}

fn json_to_map(
    value: JsonRef<'_>,
    key_type: TypeId,
    value_type: TypeId,
    store: &TypeStore,
    substitutions: &std::collections::HashMap<String, TypeId>,
) -> Option<InterpValue> {
    if let Some(object) = value.as_object() {
        return object
            .iter()
            .map(|(key, value)| {
                Some((
                    decode(JsonRef::String(key), key_type, store, substitutions)?,
                    decode(value, value_type, store, substitutions)?,
                ))
            })
            .collect::<Option<Vec<_>>>()
            .map(MapValue::new)
            .map(InterpValue::Map);
    }
    value
        .as_array()?
        .iter()
        .map(|entry| {
            if let Some(pair) = entry.as_array() {
                if pair.len() != 2 {
                    return None;
                }
                let key = pair.get(0)?;
                let value = pair.get(1)?;
                return Some((
                    decode(key, key_type, store, substitutions)?,
                    decode(value, value_type, store, substitutions)?,
                ));
            }
            let object = entry.as_object()?;
            Some((
                decode(object.get("key")?, key_type, store, substitutions)?,
                decode(object.get("value")?, value_type, store, substitutions)?,
            ))
        })
        .collect::<Option<Vec<_>>>()
        .map(MapValue::new)
        .map(InterpValue::Map)
}

fn json_to_result(
    value: JsonRef<'_>,
    ok: TypeId,
    err: TypeId,
    store: &TypeStore,
    substitutions: &std::collections::HashMap<String, TypeId>,
) -> Option<InterpValue> {
    let object = value.as_object()?;
    if let Some(value) = object.get("Ok") {
        return decode(value, ok, store, substitutions).map(|value| InterpValue::Variant {
            name: "Ok".to_owned().into(),
            fields: vec![value].into(),
        });
    }
    object
        .get("Err")
        .and_then(|value| decode(value, err, store, substitutions))
        .map(|value| InterpValue::Variant {
            name: "Err".to_owned().into(),
            fields: vec![value].into(),
        })
}

fn json_to_enum(value: JsonRef<'_>) -> Option<InterpValue> {
    if let Some(name) = value.as_str() {
        return Some(InterpValue::Variant {
            name: name.to_owned().into(),
            fields: Vec::new().into(),
        });
    }
    let object = value.as_object()?;
    let mut entries = object.iter();
    let (name, fields) = entries.next()?;
    if entries.next().is_some() {
        return None;
    }
    if !fields.is_null() && !fields.as_array().is_some_and(|values| values.is_empty()) {
        return None;
    }
    let fields = Vec::new();
    Some(InterpValue::Variant {
        name: name.into(),
        fields: fields.into(),
    })
}

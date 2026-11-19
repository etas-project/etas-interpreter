use etas_frontend::CheckedProject;
use etas_types::{Type, TypeId};

use super::super::{HostJsonSupportValue as Json, InterpValue as Value, NumericValue};

/// The public ADT and the optimized JSON tree have one observable value model.
/// Only the checked canonical JsonValue constructor may call this adapter.
pub(crate) fn construct(
    name: &str,
    fields: Vec<Value>,
    checked: &CheckedProject,
) -> Result<Json, String> {
    let invalid = || format!("JSON `{name}` constructor payload differs from its checked variant");
    let mut fields = fields.into_iter();
    let value = match (name, fields.next()) {
        ("Null", None) => Json::Null,
        ("Bool", Some(Value::Bool(value))) => Json::Bool(value),
        ("Number", Some(Value::Number(NumericValue::F64(bits)))) => Json::NumberBits(bits),
        ("String", Some(Value::String(value))) => Json::String(value),
        ("Array", Some(Value::Array(values))) => Json::Array(
            values
                .into_values()
                .into_iter()
                .map(|value| match value {
                    Value::Json(value) => Ok(value),
                    _ => Err(invalid()),
                })
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        ),
        ("Object", Some(values)) => Json::Object(object_entries(values, checked)?.into()),
        (_, value) => {
            return Err(format!(
                "{}: received {}",
                invalid(),
                value.as_ref().map_or("no payload", Value::kind_name)
            ));
        }
    };
    if fields.next().is_some() {
        return Err(invalid());
    }
    Ok(value)
}

/// Project one layer only. Recursive children retain their shared JSON storage.
pub(crate) fn project(value: &Json, checked: &CheckedProject) -> (&'static str, Vec<Value>) {
    match value {
        Json::Null => ("Null", vec![]),
        Json::Bool(value) => ("Bool", vec![Value::Bool(*value)]),
        Json::NumberBits(bits) => ("Number", vec![Value::Number(NumericValue::F64(*bits))]),
        Json::String(value) => ("String", vec![Value::String(value.clone())]),
        Json::Array(values) => (
            "Array",
            vec![Value::Array(
                values
                    .iter()
                    .cloned()
                    .map(Value::Json)
                    .collect::<Vec<_>>()
                    .into(),
            )],
        ),
        Json::Object(fields) => {
            let entries = fields.iter().map(|(key, value)| {
                (
                    Value::String(key.as_str().into()),
                    Value::Json(value.clone()),
                )
            });
            let value = match source_object_map(checked) {
                Some(ty) => {
                    let entries = Value::List(
                        entries
                            .map(|(key, value)| Value::Tuple(vec![key, value].into()))
                            .collect::<Vec<_>>()
                            .into(),
                    );
                    let entries = match source_map_entries_type(&checked.type_store, ty) {
                        Some(ty) => Value::Nominal {
                            ty,
                            value: entries.into(),
                        },
                        None => entries,
                    };
                    let fields = vec![
                        ("entries".into(), entries),
                        (
                            "length".into(),
                            Value::Number(NumericValue::usize(fields.len())),
                        ),
                    ];
                    Value::Nominal {
                        ty,
                        value: Value::Record(fields.into()).into(),
                    }
                }
                None => Value::Map(entries.collect::<Vec<_>>().into()),
            };
            ("Object", vec![value])
        }
    }
}

fn source_object_map(checked: &CheckedProject) -> Option<TypeId> {
    let json = checked.types.known_std_types.json_value?;
    let object = checked
        .types
        .enum_layouts
        .get(&json)?
        .variants
        .iter()
        .find(|variant| variant.name == "Object")?;
    let [map] = object.fields.as_slice() else {
        return None;
    };
    matches!(checked.type_store.get(*map), Some(Type::Applied { .. })).then_some(*map)
}

fn source_map_entries_type(store: &etas_types::TypeStore, map: TypeId) -> Option<TypeId> {
    let record = etas_types::ty::applied_representation_in_store(store, map).ok()??;
    let Type::Record(record) = store.get(record)? else {
        return None;
    };
    let list = record
        .fields
        .iter()
        .find(|field| field.name == "entries")?
        .ty;
    matches!(
        store.get(list),
        Some(Type::Nominal(_) | Type::Applied { .. })
    )
    .then_some(list)
}

fn object_entries(value: Value, checked: &CheckedProject) -> Result<Vec<(String, Json)>, String> {
    let invalid_entry = || "JSON Object map entry requires a string and JsonValue".into();
    match value {
        Value::Map(values) => values
            .into_values()
            .into_iter()
            .map(|(key, value)| json_entry(&key, &value))
            .collect::<Option<_>>()
            .ok_or_else(invalid_entry),
        Value::Nominal { ty, value } => {
            let expected = source_object_map(checked);
            if Some(ty) != expected {
                return Err(format!(
                    "JSON Object expects checked Map type {expected:?}, received {ty:?} {:?}",
                    checked.type_store.get(ty)
                ));
            }
            let Value::Record(fields) = value.as_ref() else {
                return Err("JSON Object's checked Map is not represented by a record".into());
            };
            let entries = match fields.get_ref("entries") {
                Some(Value::Nominal { value, .. }) => Some(value.as_ref()),
                value => value,
            };
            let Some(Value::List(entries)) = entries else {
                return Err("JSON Object's checked Map has no entries list".into());
            };
            entries
                .iter()
                .map(|entry| {
                    let Value::Tuple(pair) = entry else {
                        return None;
                    };
                    let [key, value] = pair.as_slice() else {
                        return None;
                    };
                    json_entry(key, value)
                })
                .collect::<Option<_>>()
                .ok_or_else(invalid_entry)
        }
        value => Err(format!(
            "JSON Object requires Map, received {}",
            value.kind_name()
        )),
    }
}

fn json_entry(key: &Value, value: &Value) -> Option<(String, Json)> {
    match (key, value) {
        (Value::String(key), Value::Json(value)) => Some((key.to_string(), value.clone())),
        _ => None,
    }
}

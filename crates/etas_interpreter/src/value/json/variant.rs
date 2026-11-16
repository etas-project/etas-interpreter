use super::super::{HostJsonSupportValue as Json, InterpValue as Value, NumericValue};

/// The public ADT and the optimized JSON tree have one observable value model.
/// Only the checked canonical JsonValue constructor may call this adapter.
pub(crate) fn construct(name: &str, fields: Vec<Value>) -> Result<Json, String> {
    let invalid = || "JSON constructor payload differs from its checked variant".to_owned();
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
        ("Object", Some(Value::Map(values))) => Json::Object(
            values
                .into_values()
                .into_iter()
                .map(|(key, value)| match (key, value) {
                    (Value::String(key), Value::Json(value)) => Ok((key.to_string(), value)),
                    _ => Err(invalid()),
                })
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        ),
        _ => return Err(invalid()),
    };
    if fields.next().is_some() {
        return Err(invalid());
    }
    Ok(value)
}

/// Project one layer only. Recursive children retain their shared JSON storage.
pub(crate) fn project(value: &Json) -> (&'static str, Vec<Value>) {
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
        Json::Object(fields) => (
            "Object",
            vec![Value::Map(
                fields
                    .iter()
                    .map(|(key, value)| {
                        (
                            Value::String(key.as_str().into()),
                            Value::Json(value.clone()),
                        )
                    })
                    .collect::<Vec<_>>()
                    .into(),
            )],
        ),
    }
}

use etas_host::{HostJsonValue, HostSchema, HostValue};

pub(in crate::eval) fn validate_host_value_schema(
    value: &HostValue,
    schema: &HostSchema,
    path: &str,
) -> Result<(), String> {
    if let HostValue::Json(json) = value {
        return validate_host_json_schema(json, schema, path);
    }
    match schema {
        HostSchema::Json => Ok(()),
        HostSchema::Unit => matches_schema(matches!(value, HostValue::Unit), path, "unit"),
        HostSchema::Bool => matches_schema(matches!(value, HostValue::Bool(_)), path, "bool"),
        HostSchema::Int => matches_schema(matches!(value, HostValue::Int(_)), path, "int"),
        HostSchema::UInt => matches_schema(matches!(value, HostValue::UInt(_)), path, "uint"),
        HostSchema::Float => matches_schema(matches!(value, HostValue::Float(_)), path, "float"),
        HostSchema::String => matches_schema(matches!(value, HostValue::String(_)), path, "string"),
        HostSchema::Bytes => matches_schema(matches!(value, HostValue::Bytes(_)), path, "bytes"),
        HostSchema::List(item_schema) => {
            let HostValue::List(values) = value else {
                return Err(format!("{path} must be a list"));
            };
            for (index, item) in values.iter().enumerate() {
                validate_host_value_schema(item, item_schema, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        HostSchema::Map { key, value: item } => {
            let HostValue::Map(entries) = value else {
                return Err(format!("{path} must be a map"));
            };
            for (index, (entry_key, entry_value)) in entries.iter().enumerate() {
                validate_host_value_schema(entry_key, key, &format!("{path}[{index}].key"))?;
                validate_host_value_schema(entry_value, item, &format!("{path}[{index}].value"))?;
            }
            Ok(())
        }
        HostSchema::Record(fields) => {
            let HostValue::Record(values) = value else {
                return Err(format!("{path} must be a record"));
            };
            validate_record_fields(values, fields, path, validate_host_value_schema)
        }
        HostSchema::Variant(variants) => {
            let HostValue::Variant { name, fields } = value else {
                return Err(format!("{path} must be a variant"));
            };
            let Some(variant) = variants.iter().find(|variant| variant.name == *name) else {
                return Err(format!("{path} has unknown variant `{name}`"));
            };
            if fields.len() != variant.fields.len() {
                return Err(format!(
                    "{path}.{name} expects {} field(s), got {}",
                    variant.fields.len(),
                    fields.len()
                ));
            }
            for (index, (field, schema)) in fields.iter().zip(&variant.fields).enumerate() {
                validate_host_value_schema(field, schema, &format!("{path}.{name}[{index}]"))?;
            }
            Ok(())
        }
    }
}

fn validate_record_fields<T>(
    values: &[(String, T)],
    fields: &[etas_host::HostFieldSchema],
    path: &str,
    validate: fn(&T, &HostSchema, &str) -> Result<(), String>,
) -> Result<(), String> {
    for field in fields {
        match values.iter().find(|(name, _)| name == &field.name) {
            Some((_, value)) => validate(value, &field.schema, &format!("{path}.{}", field.name))?,
            None if field.optional => {}
            None => return Err(format!("{path} is missing field `{}`", field.name)),
        }
    }
    for (name, _) in values {
        if !fields.iter().any(|field| &field.name == name) {
            return Err(format!("{path} has unexpected field `{name}`"));
        }
    }
    Ok(())
}

fn validate_host_json_schema(
    value: &HostJsonValue,
    schema: &HostSchema,
    path: &str,
) -> Result<(), String> {
    match schema {
        HostSchema::Json => Ok(()),
        HostSchema::Unit => matches_schema(matches!(value, HostJsonValue::Null), path, "unit"),
        HostSchema::Bool => matches_schema(matches!(value, HostJsonValue::Bool(_)), path, "bool"),
        HostSchema::Int => matches_schema(
            matches!(value, HostJsonValue::Number(number) if number.fract() == 0.0),
            path,
            "int",
        ),
        HostSchema::UInt => matches_schema(
            matches!(value, HostJsonValue::Number(number) if number.fract() == 0.0 && *number >= 0.0),
            path,
            "uint",
        ),
        HostSchema::Float => matches_schema(
            matches!(value, HostJsonValue::Number(number) if number.is_finite()),
            path,
            "float",
        ),
        HostSchema::String => {
            matches_schema(matches!(value, HostJsonValue::String(_)), path, "string")
        }
        HostSchema::Bytes => {
            let HostJsonValue::Array(values) = value else {
                return Err(format!("{path} must be bytes"));
            };
            for (index, byte) in values.iter().enumerate() {
                let HostJsonValue::Number(number) = byte else {
                    return Err(format!("{path}[{index}] must be a byte"));
                };
                if number.fract() != 0.0 || !(0.0..=255.0).contains(number) {
                    return Err(format!("{path}[{index}] must be a byte"));
                }
            }
            Ok(())
        }
        HostSchema::List(item_schema) => {
            let HostJsonValue::Array(values) = value else {
                return Err(format!("{path} must be a list"));
            };
            for (index, item) in values.iter().enumerate() {
                validate_host_json_schema(item, item_schema, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        HostSchema::Map { key, value: item } => {
            let HostJsonValue::Array(entries) = value else {
                return Err(format!("{path} must be a map entry list"));
            };
            for (index, entry) in entries.iter().enumerate() {
                let HostJsonValue::Array(pair) = entry else {
                    return Err(format!("{path}[{index}] must be a [key, value] pair"));
                };
                if pair.len() != 2 {
                    return Err(format!("{path}[{index}] must be a [key, value] pair"));
                }
                validate_host_json_schema(&pair[0], key, &format!("{path}[{index}].key"))?;
                validate_host_json_schema(&pair[1], item, &format!("{path}[{index}].value"))?;
            }
            Ok(())
        }
        HostSchema::Record(fields) => {
            let HostJsonValue::Object(values) = value else {
                return Err(format!("{path} must be a record"));
            };
            validate_record_fields(values, fields, path, validate_host_json_schema)
        }
        HostSchema::Variant(variants) => {
            let HostJsonValue::Object(values) = value else {
                return Err(format!("{path} must be a variant object"));
            };
            let Some(HostJsonValue::String(name)) = values
                .iter()
                .find(|(field, _)| field == "name")
                .map(|(_, value)| value)
            else {
                return Err(format!("{path} variant is missing string field `name`"));
            };
            let Some(HostJsonValue::Array(fields)) = values
                .iter()
                .find(|(field, _)| field == "fields")
                .map(|(_, value)| value)
            else {
                return Err(format!("{path} variant is missing array field `fields`"));
            };
            let Some(variant) = variants.iter().find(|variant| variant.name == *name) else {
                return Err(format!("{path} has unknown variant `{name}`"));
            };
            if fields.len() != variant.fields.len() {
                return Err(format!(
                    "{path}.{name} expects {} field(s), got {}",
                    variant.fields.len(),
                    fields.len()
                ));
            }
            for (index, (field, schema)) in fields.iter().zip(&variant.fields).enumerate() {
                validate_host_json_schema(field, schema, &format!("{path}.{name}[{index}]"))?;
            }
            Ok(())
        }
    }
}

fn matches_schema(matches: bool, path: &str, expected: &str) -> Result<(), String> {
    if matches {
        Ok(())
    } else {
        Err(format!("{path} must be {expected}"))
    }
}

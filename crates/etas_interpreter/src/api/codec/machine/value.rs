use serde_json::Value;

use crate::api::codec::value_from_json;

pub(super) fn optional_values(
    value: &Value,
    field: &str,
) -> Result<Option<Vec<crate::value::InterpValue>>, String> {
    let Some(value) = value.get(field) else {
        return Err(format!("machine snapshot is missing `{field}`"));
    };
    if value.is_null() {
        return Ok(None);
    }
    let values = value
        .as_array()
        .ok_or_else(|| format!("machine snapshot `{field}` must be an array or null"))?;
    values
        .iter()
        .map(|value| value_from_json(value).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

pub(super) fn optional_value(
    value: &Value,
    field: &str,
) -> Result<Option<crate::value::InterpValue>, String> {
    let Some(value) = value.get(field) else {
        return Err(format!("machine snapshot is missing `{field}`"));
    };
    if value.is_null() {
        return Ok(None);
    }
    value_from_json(value)
        .map(Some)
        .map_err(|error| error.to_string())
}

pub(super) fn required_values(
    value: &Value,
    field: &str,
) -> Result<Vec<crate::value::InterpValue>, String> {
    required(value, field)?
        .as_array()
        .ok_or_else(|| format!("machine snapshot `{field}` must be an array"))?
        .iter()
        .map(|value| value_from_json(value).map_err(|error| error.to_string()))
        .collect()
}

pub(super) fn required<'a>(value: &'a Value, field: &str) -> Result<&'a Value, String> {
    value
        .get(field)
        .ok_or_else(|| format!("machine snapshot is missing `{field}`"))
}

pub(super) fn required_str<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    required(value, field)?
        .as_str()
        .ok_or_else(|| format!("machine snapshot `{field}` must be a string"))
}

pub(super) fn required_bool(value: &Value, field: &str) -> Result<bool, String> {
    required(value, field)?
        .as_bool()
        .ok_or_else(|| format!("machine snapshot `{field}` must be a bool"))
}

pub(super) fn required_u32(value: &Value, field: &str) -> Result<u32, String> {
    required(value, field)?
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("machine snapshot `{field}` must be a u32"))
}

pub(super) fn required_u64(value: &Value, field: &str) -> Result<u64, String> {
    required(value, field)?
        .as_u64()
        .ok_or_else(|| format!("machine snapshot `{field}` must be a u64"))
}

pub(super) fn required_usize(value: &Value, field: &str) -> Result<usize, String> {
    required(value, field)?
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("machine snapshot `{field}` must be a usize"))
}

pub(super) fn optional_u32(value: &Value, field: &str) -> Result<Option<u32>, String> {
    let Some(value) = value.get(field) else {
        return Err(format!("machine snapshot is missing `{field}`"));
    };
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .map(Some)
        .ok_or_else(|| format!("machine snapshot `{field}` must be a u32 or null"))
}

pub(super) fn optional_string(value: &Value, field: &str) -> Result<Option<String>, String> {
    let Some(value) = value.get(field) else {
        return Err(format!("machine snapshot is missing `{field}`"));
    };
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_str()
        .map(|value| Some(value.to_owned()))
        .ok_or_else(|| format!("machine snapshot `{field}` must be a string or null"))
}

pub(super) fn required_u32_array(value: &Value, field: &str) -> Result<Vec<u32>, String> {
    required(value, field)?
        .as_array()
        .ok_or_else(|| format!("machine snapshot `{field}` must be an array"))?
        .iter()
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| format!("machine snapshot `{field}` must contain u32 values"))
        })
        .collect()
}

pub(super) fn required_string_array(value: &Value, field: &str) -> Result<Vec<String>, String> {
    required(value, field)?
        .as_array()
        .ok_or_else(|| format!("machine snapshot `{field}` must be an array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("machine snapshot `{field}` must contain strings"))
        })
        .collect()
}

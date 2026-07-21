use super::*;

pub(super) fn string_array(
    value: &Value,
    field: &'static str,
) -> Result<Vec<String>, InterpreterCodecError> {
    required_array(value, field)?
        .iter()
        .map(|item| {
            item.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                InterpreterCodecError::new(format!("`{field}` must contain strings"))
            })
        })
        .collect()
}

pub(super) fn u32_array(
    value: &Value,
    field: &'static str,
) -> Result<Vec<u32>, InterpreterCodecError> {
    required_array(value, field)?
        .iter()
        .map(|item| {
            item.as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| {
                    InterpreterCodecError::new(format!("`{field}` must contain u32 values"))
                })
        })
        .collect()
}

pub(super) fn byte_array(
    value: &Value,
    field: &'static str,
) -> Result<Vec<u8>, InterpreterCodecError> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| InterpreterCodecError::new(format!("`{field}` must be a byte array")))?
        .iter()
        .map(|byte| {
            byte.as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .ok_or_else(|| {
                    InterpreterCodecError::new(format!("`{field}` contains an invalid byte value"))
                })
        })
        .collect()
}

pub(super) fn required_obj<'a>(
    value: &'a Value,
    field: &'static str,
) -> Result<&'a Value, InterpreterCodecError> {
    value
        .get(field)
        .ok_or_else(|| InterpreterCodecError::new(format!("missing `{field}`")))
}

pub(super) fn required_array<'a>(
    value: &'a Value,
    field: &'static str,
) -> Result<&'a [Value], InterpreterCodecError> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| InterpreterCodecError::new(format!("missing array `{field}`")))
}

pub(super) fn required_str<'a>(
    value: &'a Value,
    field: &'static str,
) -> Result<&'a str, InterpreterCodecError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| InterpreterCodecError::new(format!("missing string `{field}`")))
}

pub(super) fn optional_string(
    value: &Value,
    field: &'static str,
) -> Result<Option<String>, InterpreterCodecError> {
    let Some(raw) = value.get(field) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    raw.as_str()
        .map(|value| Some(value.to_owned()))
        .ok_or_else(|| InterpreterCodecError::new(format!("invalid optional string `{field}`")))
}

pub(super) fn required_optional_string(
    value: &Value,
    field: &'static str,
) -> Result<Option<String>, InterpreterCodecError> {
    let raw = value
        .get(field)
        .ok_or_else(|| InterpreterCodecError::new(format!("missing `{field}`")))?;
    if raw.is_null() {
        return Ok(None);
    }
    raw.as_str()
        .map(|value| Some(value.to_owned()))
        .ok_or_else(|| InterpreterCodecError::new(format!("invalid optional string `{field}`")))
}

pub(super) fn required_bool(
    value: &Value,
    field: &'static str,
) -> Result<bool, InterpreterCodecError> {
    value
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| InterpreterCodecError::new(format!("missing bool `{field}`")))
}

pub(super) fn required_i64(
    value: &Value,
    field: &'static str,
) -> Result<i64, InterpreterCodecError> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| InterpreterCodecError::new(format!("missing integer `{field}`")))
}

pub(super) fn required_u32(
    value: &Value,
    field: &'static str,
) -> Result<u32, InterpreterCodecError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| InterpreterCodecError::new(format!("missing u32 `{field}`")))
}

pub(super) fn required_u64(
    value: &Value,
    field: &'static str,
) -> Result<u64, InterpreterCodecError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| InterpreterCodecError::new(format!("missing u64 `{field}`")))
}

pub(super) fn optional_u32(
    value: &Value,
    field: &'static str,
) -> Result<Option<u32>, InterpreterCodecError> {
    let Some(raw) = value.get(field) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    raw.as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .map(Some)
        .ok_or_else(|| InterpreterCodecError::new(format!("invalid optional u32 `{field}`")))
}

pub(super) fn required_usize(
    value: &Value,
    field: &'static str,
) -> Result<usize, InterpreterCodecError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| InterpreterCodecError::new(format!("missing usize `{field}`")))
}

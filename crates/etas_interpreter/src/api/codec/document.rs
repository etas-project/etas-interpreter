use std::{io, ops::Deref};

use serde_json::Value;

#[cfg(test)]
mod tests;
mod write;

/// Owned JSON with iterative clone, comparison, output and release.
/// Checkpoint and run-report codecs share this owner without live mutable state.
pub struct JsonDocument(pub(in crate::api::codec) Value);

impl std::fmt::Debug for JsonDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonDocument")
            .field("schema", &self.0.get("schema").and_then(Value::as_str))
            .field("flow", &self.0.get("flow").and_then(Value::as_str))
            .finish_non_exhaustive()
    }
}

impl PartialEq for JsonDocument {
    fn eq(&self, other: &Self) -> bool {
        self.eq_value(&other.0)
    }
}

impl PartialEq<Value> for JsonDocument {
    fn eq(&self, other: &Value) -> bool {
        self.eq_value(other)
    }
}

impl PartialEq<JsonDocument> for Value {
    fn eq(&self, other: &JsonDocument) -> bool {
        other.eq_value(self)
    }
}

impl JsonDocument {
    fn eq_value(&self, other: &Value) -> bool {
        let mut pending = vec![(&self.0, other)];
        while let Some((left, right)) = pending.pop() {
            match (left, right) {
                (Value::Array(a), Value::Array(b)) => {
                    if a.len() != b.len() {
                        return false;
                    }
                    pending.extend(a.iter().zip(b));
                }
                (Value::Object(a), Value::Object(b)) => {
                    if a.len() != b.len() {
                        return false;
                    }
                    for ((ak, av), (bk, bv)) in a.iter().zip(b) {
                        if ak != bk {
                            return false;
                        }
                        pending.push((av, bv));
                    }
                }
                (a, b) if a == b => (),
                _ => return false,
            }
        }
        true
    }
}

impl JsonDocument {
    pub(in crate::api::codec) fn from_value(value: Value) -> Self {
        Self(value)
    }

    pub(in crate::api::codec) fn value_mut(&mut self) -> &mut Value {
        &mut self.0
    }

    pub(in crate::api::codec) fn into_value(mut self) -> Value {
        std::mem::take(&mut self.0)
    }
}

impl Deref for JsonDocument {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.0
    }
}

impl std::ops::DerefMut for JsonDocument {
    fn deref_mut(&mut self) -> &mut Value {
        &mut self.0
    }
}

impl From<Value> for JsonDocument {
    fn from(value: Value) -> Self {
        Self(value)
    }
}

impl Clone for JsonDocument {
    fn clone(&self) -> Self {
        let mut output = Self(Value::Null);
        let mut pending = vec![(&self.0, &mut output.0)];
        while let Some((input, slot)) = pending.pop() {
            match input {
                Value::Array(values) => {
                    *slot = Value::Array((0..values.len()).map(|_| Value::Null).collect());
                    if let Value::Array(slots) = slot {
                        pending.extend(values.iter().zip(slots));
                    }
                }
                Value::Object(fields) => {
                    *slot = Value::Object(
                        fields
                            .keys()
                            .map(|key| (key.clone(), Value::Null))
                            .collect(),
                    );
                    if let Value::Object(slots) = slot {
                        pending.extend(fields.values().zip(slots.values_mut()));
                    }
                }
                input => *slot = input.clone(),
            }
        }
        output
    }
}

impl Drop for JsonDocument {
    fn drop(&mut self) {
        release_json(std::mem::take(&mut self.0));
    }
}

pub(in crate::api::codec) fn release_json(root: Value) {
    if !matches!(root, Value::Array(_) | Value::Object(_)) {
        return;
    }
    let mut pending = vec![root];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(children) => pending.extend(children),
            Value::Object(fields) => pending.extend(fields.into_values()),
            _ => {}
        }
    }
}

impl JsonDocument {
    pub fn write_to(&self, output: impl io::Write) -> io::Result<()> {
        write_json(output, &self.0)
    }
}

/// Writes a borrowed subtree without cloning it or recursing through JSON storage.
pub fn write_json(output: impl io::Write, value: &Value) -> io::Result<()> {
    write::json(output, value)
}

impl std::fmt::Display for JsonDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct Output<'a, 'b>(&'a mut std::fmt::Formatter<'b>);
        impl io::Write for Output<'_, '_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
                self.0.write_str(text).map_err(io::Error::other)?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        self.write_to(Output(f)).map_err(|_| std::fmt::Error)
    }
}

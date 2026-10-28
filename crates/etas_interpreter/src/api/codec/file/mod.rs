use std::ops::Deref;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::InterpreterCodecError;

mod bounded;
mod decode;
mod encode;
#[cfg(test)]
mod tests;

pub use decode::checkpoint_file_from_bytes;
pub use encode::checkpoint_file_to_bytes;

const FILE_SCHEMA: &str = "etas.interpreter.checkpoint-file.flat-json.v1";

/// File resource budgets, independent of source recursion and storage payload limits.
#[derive(Clone, Copy, Debug)]
pub struct CheckpointFileLimits {
    pub max_bytes: usize,
    pub max_nodes: usize,
}

impl Default for CheckpointFileLimits {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024 * 1024,
            max_nodes: 1_000_000,
        }
    }
}

impl CheckpointFileLimits {
    fn validate(self) -> Result<(), InterpreterCodecError> {
        if self.max_bytes == 0 || self.max_nodes == 0 {
            return Err(InterpreterCodecError::new(
                "checkpoint file budgets must be nonzero",
            ));
        }
        Ok(())
    }
}

/// An owned logical artifact whose recursive JSON storage is released iteratively.
/// Borrow it for checked decoding; it never aliases a live evaluator frame.
pub struct CheckpointDocument(Value);

impl std::fmt::Debug for CheckpointDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckpointDocument")
            .field("schema", &self.0.get("schema").and_then(Value::as_str))
            .field("flow", &self.0.get("flow").and_then(Value::as_str))
            .finish_non_exhaustive()
    }
}

impl PartialEq for CheckpointDocument {
    fn eq(&self, other: &Self) -> bool {
        let mut pending = vec![(&self.0, &other.0)];
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

impl CheckpointDocument {
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

impl Deref for CheckpointDocument {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.0
    }
}

impl std::ops::DerefMut for CheckpointDocument {
    fn deref_mut(&mut self) -> &mut Value {
        &mut self.0
    }
}

impl From<Value> for CheckpointDocument {
    fn from(value: Value) -> Self {
        Self(value)
    }
}

impl Clone for CheckpointDocument {
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

impl Drop for CheckpointDocument {
    fn drop(&mut self) {
        release_json(std::mem::take(&mut self.0));
    }
}

struct JsonSlots(Vec<Value>);

impl Drop for JsonSlots {
    fn drop(&mut self) {
        for value in std::mem::take(&mut self.0) {
            release_json(value);
        }
    }
}

fn release_json(root: Value) {
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

// Indices encode a tree, not deduplicated runtime identities. Every non-root
// node has exactly one earlier parent; aliases, cycles and orphans are invalid.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    schema: String,
    root: usize,
    nodes: Vec<Node>,
}

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum Node {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<usize>),
    Object(Vec<(String, usize)>),
}

fn check_document_schema(value: &Value) -> Result<(), InterpreterCodecError> {
    let schema = super::required_str(value, "schema")?;
    if schema != crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA {
        return Err(InterpreterCodecError::new(format!(
            "unsupported checkpoint artifact schema `{schema}`; expected `{}`",
            crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA,
        )));
    }
    Ok(())
}

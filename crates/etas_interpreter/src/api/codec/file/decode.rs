use serde::de::DeserializeSeed;
use std::collections::HashSet;

use super::*;

/// Parses only the bounded-depth file envelope, then rebuilds the logical tree
/// iteratively. Call checkpoint_from_json_with_limits for checked HIR validation.
pub fn checkpoint_file_from_bytes(
    bytes: &[u8],
    limits: CheckpointFileLimits,
) -> Result<CheckpointDocument, InterpreterCodecError> {
    limits.validate()?;
    if bytes.len() > limits.max_bytes {
        return Err(InterpreterCodecError::new(
            "checkpoint file exceeds byte budget",
        ));
    }
    #[derive(Deserialize)]
    struct Version {
        schema: String,
    }
    let version: Version = serde_json::from_slice(bytes)
        .map_err(|error| InterpreterCodecError::new(format!("invalid checkpoint file: {error}")))?;
    if version.schema != FILE_SCHEMA {
        return Err(InterpreterCodecError::new(format!(
            "unsupported checkpoint file schema `{}`; expected `{FILE_SCHEMA}`",
            version.schema,
        )));
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let file = super::bounded::FileSeed(limits.max_nodes)
        .deserialize(&mut decoder)
        .map_err(|error| InterpreterCodecError::new(format!("invalid checkpoint file: {error}")))?;
    decoder
        .end()
        .map_err(|error| InterpreterCodecError::new(format!("invalid checkpoint file: {error}")))?;
    validate_tree(&file, limits)?;
    let mut slots = JsonSlots((0..file.nodes.len()).map(|_| Value::Null).collect());
    for (index, node) in file.nodes.into_iter().enumerate().rev() {
        slots.0[index] = match node {
            Node::Null => Value::Null,
            Node::Bool(value) => Value::Bool(value),
            Node::Number(value) => Value::Number(value),
            Node::String(value) => Value::String(value),
            Node::Array(children) => Value::Array(
                children
                    .into_iter()
                    .map(|child| std::mem::take(&mut slots.0[child]))
                    .collect(),
            ),
            Node::Object(fields) => Value::Object(
                fields
                    .into_iter()
                    .map(|(name, child)| (name, std::mem::take(&mut slots.0[child])))
                    .collect(),
            ),
        };
    }
    let document = CheckpointDocument(std::mem::take(&mut slots.0[file.root]));
    check_document_schema(&document)?;
    Ok(document)
}

fn validate_tree(file: &File, limits: CheckpointFileLimits) -> Result<(), InterpreterCodecError> {
    if file.root != 0 || file.nodes.is_empty() {
        return Err(InterpreterCodecError::new(
            "checkpoint file must have root node 0",
        ));
    }
    if file.nodes.len() > limits.max_nodes {
        return Err(InterpreterCodecError::new(
            "checkpoint file exceeds node budget",
        ));
    }
    let mut owned = vec![false; file.nodes.len()];
    owned[0] = true;
    for (parent, node) in file.nodes.iter().enumerate() {
        let mut claim = |child: usize| {
            if child <= parent || child >= owned.len() {
                return Err(InterpreterCodecError::new(
                    "invalid checkpoint node reference order or index",
                ));
            }
            if std::mem::replace(&mut owned[child], true) {
                return Err(InterpreterCodecError::new(
                    "checkpoint node has multiple owners",
                ));
            }
            Ok(())
        };
        match node {
            Node::Array(children) => {
                for child in children {
                    claim(*child)?;
                }
            }
            Node::Object(fields) => {
                let mut names = HashSet::new();
                for (name, child) in fields {
                    if !names.insert(name) {
                        return Err(InterpreterCodecError::new(
                            "duplicate checkpoint object field",
                        ));
                    }
                    claim(*child)?;
                }
            }
            _ => {}
        }
    }
    if owned.iter().any(|owned| !owned) {
        return Err(InterpreterCodecError::new(
            "checkpoint file contains orphan nodes",
        ));
    }
    Ok(())
}

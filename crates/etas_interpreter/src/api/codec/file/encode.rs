use std::io::{self, Write};

use super::*;

/// Consumes a logical artifact without cloning its recursive JSON tree.
/// The file envelope has a separate version from the checked machine schema.
pub fn checkpoint_file_to_bytes(
    artifact: Value,
    limits: CheckpointFileLimits,
) -> Result<Vec<u8>, InterpreterCodecError> {
    let mut artifact = CheckpointDocument(artifact);
    limits.validate()?;
    check_document_schema(&artifact)?;
    let mut pending = JsonSlots(vec![std::mem::take(&mut artifact.0)]);
    let mut nodes = Vec::new();
    let mut index = 0;
    while index < pending.0.len() {
        let value = CheckpointDocument(std::mem::take(&mut pending.0[index]));
        let child_count = match &value.0 {
            Value::Array(values) => values.len(),
            Value::Object(fields) => fields.len(),
            _ => 0,
        };
        if pending
            .0
            .len()
            .checked_add(child_count)
            .is_none_or(|n| n > limits.max_nodes)
        {
            return Err(InterpreterCodecError::new(
                "checkpoint file exceeds node budget",
            ));
        }
        let mut value = value;
        let node = match std::mem::take(&mut value.0) {
            Value::Null => Node::Null,
            Value::Bool(value) => Node::Bool(value),
            Value::Number(value) => Node::Number(value),
            Value::String(value) => Node::String(value),
            Value::Array(values) => Node::Array(
                values
                    .into_iter()
                    .map(|value| {
                        let child = pending.0.len();
                        pending.0.push(value);
                        child
                    })
                    .collect(),
            ),
            Value::Object(fields) => Node::Object(
                fields
                    .into_iter()
                    .map(|(key, value)| {
                        let child = pending.0.len();
                        pending.0.push(value);
                        (key, child)
                    })
                    .collect(),
            ),
        };
        nodes.push(node);
        index += 1;
    }
    let file = File {
        schema: FILE_SCHEMA.to_owned(),
        root: 0,
        nodes,
    };
    let mut output = BoundedBytes {
        bytes: Vec::new(),
        limit: limits.max_bytes,
    };
    serde_json::to_writer(&mut output, &file).map_err(|error| {
        InterpreterCodecError::new(format!("cannot encode checkpoint file: {error}"))
    })?;
    Ok(output.bytes)
}

struct BoundedBytes {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > self.limit)
        {
            return Err(io::Error::other("checkpoint file exceeds byte budget"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

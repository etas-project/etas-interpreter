use std::{
    cell::RefCell,
    collections::VecDeque,
    io::{self, Write},
};

use serde::ser::{Error, SerializeSeq, SerializeStruct};

use super::*;

/// Consumes a logical artifact without cloning its recursive JSON tree.
/// The file envelope has a separate version from the checked machine schema.
pub fn checkpoint_file_to_bytes(
    artifact: Value,
    limits: CheckpointFileLimits,
) -> Result<Vec<u8>, InterpreterCodecError> {
    let artifact = CheckpointDocument(artifact);
    limits.validate()?;
    check_document_schema(&artifact)?;
    let nodes = FlatNodes {
        root: RefCell::new(Some(artifact)),
        max_nodes: limits.max_nodes,
    };
    let mut output = BoundedBytes {
        bytes: Vec::new(),
        limit: limits.max_bytes,
    };
    let encode = (|| -> Result<(), serde_json::Error> {
        let mut serializer = serde_json::Serializer::new(&mut output);
        let mut file = serde::Serializer::serialize_struct(&mut serializer, "File", 3)?;
        file.serialize_field("schema", FILE_SCHEMA)?;
        file.serialize_field("root", &0usize)?;
        file.serialize_field("nodes", &nodes)?;
        SerializeStruct::end(file)
    })();
    encode.map_err(|error| {
        InterpreterCodecError::new(format!("cannot encode checkpoint file: {error}"))
    })?;
    Ok(output.bytes)
}

// Serialization consumes the document once. Only the unprocessed frontier is
// retained; node IDs count all discovered nodes, not the queue's current length.
struct FlatNodes {
    root: RefCell<Option<CheckpointDocument>>,
    max_nodes: usize,
}

struct PendingValues(VecDeque<Value>);

impl Drop for PendingValues {
    fn drop(&mut self) {
        for value in self.0.drain(..) {
            release_json(value);
        }
    }
}

impl Serialize for FlatNodes {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut root = self
            .root
            .borrow_mut()
            .take()
            .ok_or_else(|| S::Error::custom("checkpoint document already consumed"))?;
        let mut pending = PendingValues(VecDeque::from([std::mem::take(&mut root.0)]));
        let mut next_id = 1usize;
        let mut sequence = serializer.serialize_seq(None)?;
        while let Some(value) = pending.0.pop_front() {
            let value = CheckpointDocument(value);
            let child_count = match &value.0 {
                Value::Array(values) => values.len(),
                Value::Object(fields) => fields.len(),
                _ => 0,
            };
            if next_id
                .checked_add(child_count)
                .is_none_or(|n| n > self.max_nodes)
            {
                return Err(S::Error::custom("checkpoint file exceeds node budget"));
            }
            pending.0.reserve(child_count);
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
                            let child = next_id;
                            next_id += 1;
                            pending.0.push_back(value);
                            child
                        })
                        .collect(),
                ),
                Value::Object(fields) => Node::Object(
                    fields
                        .into_iter()
                        .map(|(key, value)| {
                            let child = next_id;
                            next_id += 1;
                            pending.0.push_back(value);
                            (key, child)
                        })
                        .collect(),
                ),
            };
            sequence.serialize_element(&node)?;
        }
        sequence.end()
    }
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

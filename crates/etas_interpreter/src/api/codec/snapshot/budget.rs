use super::Node;
use crate::api::codec::{CheckpointFileLimits, InterpreterCodecError};
use crate::orchestration::{CallTargetSnapshot, ValueSnapshot};
use crate::value::HostJsonSupportValue as Json;

// A lower bound on logical JSON work. Charge each expanded occurrence, not
// each shared backing once. The file writer still checks exact JSON nodes and
// encoded bytes. Raw string/key/byte payloads are charged before copying;
// this does not count every metadata field or serialized escape byte.
pub(in crate::api::codec) struct EncodingBudget {
    remaining: usize,
    remaining_bytes: usize,
}

impl Default for EncodingBudget {
    fn default() -> Self {
        Self::with_limits(CheckpointFileLimits::default())
    }
}

impl EncodingBudget {
    #[cfg(test)]
    pub(in crate::api::codec) fn new(max_nodes: usize) -> Self {
        Self::with_limits(CheckpointFileLimits {
            max_nodes,
            ..Default::default()
        })
    }

    pub(in crate::api::codec) fn with_limits(limits: CheckpointFileLimits) -> Self {
        Self {
            remaining: limits.max_nodes,
            remaining_bytes: limits.max_bytes,
        }
    }

    pub(super) fn admit(
        &mut self,
        node: Node<'_>,
        pending: usize,
    ) -> Result<(), InterpreterCodecError> {
        self.remaining = self.remaining.checked_sub(1).ok_or_else(exhausted)?;
        let (children, inline_nodes) = match node {
            Node::Value(value) => match value {
                ValueSnapshot::Tuple(values)
                | ValueSnapshot::Array(values)
                | ValueSnapshot::List(values)
                | ValueSnapshot::Slice(values)
                | ValueSnapshot::Set(values)
                | ValueSnapshot::Deque(values)
                | ValueSnapshot::Queue(values)
                | ValueSnapshot::Stack(values)
                | ValueSnapshot::OrderedSet(values)
                | ValueSnapshot::Variant { fields: values, .. } => (values.len(), 0),
                ValueSnapshot::Map(values)
                | ValueSnapshot::OrderedMap(values)
                | ValueSnapshot::PriorityQueue(values) => {
                    (values.len().checked_mul(2).ok_or_else(exhausted)?, 0)
                }
                ValueSnapshot::Record(fields) => (fields.len(), 0),
                ValueSnapshot::Conversation(value) => (value.messages.len(), 0),
                ValueSnapshot::Bytes(bytes) => (0, bytes.len()),
                ValueSnapshot::Prompt(messages) => (0, messages.len()),
                ValueSnapshot::Json(_) => (1, 0),
                _ => (0, 0),
            },
            Node::Json(value) => match value {
                Json::Array(values) => (values.len(), 0),
                Json::Object(entries) => (entries.len(), 0),
                _ => (0, 0),
            },
            Node::Frame(frame) => (frame.locals.len(), frame.type_bindings.len()),
            Node::CallTarget(CallTargetSnapshot::Composed(targets)) => (targets.len(), 0),
            Node::CallTarget(CallTargetSnapshot::Specialized { type_bindings, .. }) => {
                (1, type_bindings.len())
            }
            Node::CallTarget(CallTargetSnapshot::Limited { limits, .. }) => (1, limits.len()),
            Node::CallTarget(
                CallTargetSnapshot::PureIntrinsic {
                    parameter_types, ..
                }
                | CallTargetSnapshot::StdIntrinsic {
                    parameter_types, ..
                },
            ) => (0, parameter_types.len()),
            Node::CallTargets(values) => (values.len(), 0),
            Node::Values(values) => (values.len(), 0),
            Node::NamedValues(values) => (values.len(), 0),
            Node::Pairs(values) => (values.len().checked_mul(2).ok_or_else(exhausted)?, 0),
            Node::LocalSegments(values) => (0, values.len()),
            _ => (0, 0),
        };
        self.remaining = self
            .remaining
            .checked_sub(inline_nodes)
            .ok_or_else(exhausted)?;
        if pending
            .checked_add(children)
            .is_none_or(|count| count > self.remaining)
        {
            return Err(exhausted());
        }
        match node {
            Node::Value(ValueSnapshot::String(value)) | Node::Json(Json::String(value)) => {
                self.charge_bytes(value.len())?;
            }
            Node::Value(ValueSnapshot::Bytes(value)) => self.charge_bytes(value.len())?,
            Node::Value(ValueSnapshot::Prompt(messages)) => {
                for message in messages.iter() {
                    self.charge_bytes(message.text.len())?;
                }
            }
            Node::Value(ValueSnapshot::Record(fields)) => {
                for (name, _) in fields.iter() {
                    self.charge_bytes(name.len())?;
                }
            }
            Node::NamedValues(fields) => {
                for (name, _) in fields.iter() {
                    self.charge_bytes(name.len())?;
                }
            }
            Node::Json(Json::Object(entries)) => {
                for (key, _) in entries.iter() {
                    self.charge_bytes(key.len())?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn charge_bytes(&mut self, bytes: usize) -> Result<(), InterpreterCodecError> {
        self.remaining_bytes = self.remaining_bytes.checked_sub(bytes).ok_or_else(|| {
            InterpreterCodecError::new("checkpoint snapshot payload exceeds byte budget")
        })?;
        Ok(())
    }
}

fn exhausted() -> InterpreterCodecError {
    InterpreterCodecError::new("checkpoint snapshot expansion exceeds node budget")
}

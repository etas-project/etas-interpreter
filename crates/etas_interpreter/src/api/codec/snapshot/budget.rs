use super::Node;
use crate::api::codec::{CheckpointFileLimits, InterpreterCodecError};
use crate::orchestration::{CallTargetSnapshot, ValueSnapshot};

// A lower bound on logical JSON work. Charge each expanded occurrence, not
// each shared backing once. The file writer still checks exact JSON nodes and
// encoded bytes; this gate prevents unbounded expansion before it is reached.
pub(in crate::api::codec) struct EncodingBudget {
    remaining: usize,
}

impl Default for EncodingBudget {
    fn default() -> Self {
        Self::new(CheckpointFileLimits::default().max_nodes)
    }
}

impl EncodingBudget {
    pub(in crate::api::codec) fn new(max_nodes: usize) -> Self {
        Self {
            remaining: max_nodes,
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
        Ok(())
    }
}

fn exhausted() -> InterpreterCodecError {
    InterpreterCodecError::new("checkpoint snapshot expansion exceeds node budget")
}

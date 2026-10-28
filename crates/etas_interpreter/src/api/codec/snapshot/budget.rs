use super::Node;
use crate::api::codec::value::encode_model::Part;
use crate::api::codec::{CheckpointFileLimits, InterpreterCodecError};
use crate::orchestration::{CallTargetSnapshot, ValueSnapshot};
use crate::value::HostJsonSupportValue as Json;
use crate::value::{HostSupportValue as Host, ModelContentValue};

// A lower bound on logical JSON work. Charge each expanded occurrence, not
// each shared backing once. The file writer still checks exact JSON nodes and
// encoded bytes. Raw string/key/byte payloads are charged before copying;
// this does not count every metadata field or serialized escape byte.
pub(in crate::api::codec) struct EncodingBudget {
    remaining: usize,
    remaining_bytes: usize,
    suspended_programs: Option<usize>,
}

impl Default for EncodingBudget {
    fn default() -> Self {
        Self::with_limits(CheckpointFileLimits::default())
    }
}

impl EncodingBudget {
    pub(in crate::api::codec) fn register_suspensions(&mut self, count: usize) {
        self.suspended_programs = Some(count);
    }
    pub(in crate::api::codec) fn metadata(
        &mut self,
        nodes: usize,
        bytes: usize,
    ) -> Result<(), InterpreterCodecError> {
        self.remaining = self.remaining.checked_sub(nodes).ok_or_else(exhausted)?;
        self.charge_bytes(bytes)
    }
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
            suspended_programs: None,
        }
    }

    pub(in crate::api::codec) fn admit(
        &mut self,
        node: Node<'_>,
        pending: usize,
    ) -> Result<(), InterpreterCodecError> {
        self.remaining = self.remaining.checked_sub(1).ok_or_else(exhausted)?;
        match node {
            Node::Frame(frame) => self.effect_rows(&frame.effect_row_bindings)?,
            Node::CallTarget(CallTargetSnapshot::Specialized {
                effect_row_bindings,
                ..
            }) => self.effect_rows(effect_row_bindings)?,
            _ => {}
        }
        let (children, inline_nodes) = match node {
            Node::CallSite(Some(site)) => (1 + usize::from(site.composition.is_some()), 3),
            Node::CallSite(None) => (0, 0),
            Node::Composition(composition) => (1, composition.path.len() + 2),
            Node::Continuation(crate::orchestration::ContinuationSnapshot::ProgramResume {
                id,
            }) => {
                if *id == 0
                    || self
                        .suspended_programs
                        .is_none_or(|count| *id as usize > count)
                {
                    return Err(InterpreterCodecError::new(
                        "continuation references a missing suspended program",
                    ));
                }
                (0, 0)
            }
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
                ValueSnapshot::ModelResponse(value) => model_work(Part::Response(value))?,
                _ => (0, 0),
            },
            Node::Json(value) => match value {
                Json::Array(values) => (values.len(), 0),
                Json::Object(entries) => (entries.len(), 0),
                _ => (0, 0),
            },
            Node::Host(value) => match value {
                Host::List(values) | Host::Variant { fields: values, .. } => (values.len(), 0),
                Host::Map(entries) => (entries.len().checked_mul(2).ok_or_else(exhausted)?, 0),
                Host::Record(fields) => (fields.len(), 0),
                Host::Bytes(bytes) => (0, bytes.len()),
                Host::Json(_) => (1, 0),
                Host::Unit
                | Host::Bool(_)
                | Host::Int(_)
                | Host::UInt(_)
                | Host::FloatBits(_)
                | Host::String(_) => (0, 0),
            },
            Node::Model(part) => model_work(part)?,
            Node::Frame(frame) => (frame.locals.len(), frame.type_bindings.len()),
            Node::CallTarget(CallTargetSnapshot::Composed(targets)) => (targets.len(), 0),
            Node::CallTarget(CallTargetSnapshot::Scoped { metadata, .. }) => {
                (1, metadata.projections.len())
            }
            Node::CallTarget(CallTargetSnapshot::Specialized { type_bindings, .. }) => {
                (1, type_bindings.len())
            }
            Node::CallTarget(CallTargetSnapshot::Limited { limits, .. }) => (1, limits.len()),
            Node::CallTarget(CallTargetSnapshot::PureIntrinsic {
                parameter_types, ..
            }) => (0, parameter_types.len()),
            Node::CallTarget(CallTargetSnapshot::StdIntrinsic {
                parameter_types,
                substitutions,
                ..
            }) => {
                let nodes = substitutions.iter().try_fold(
                    parameter_types
                        .len()
                        .checked_add(substitutions.len())
                        .ok_or_else(exhausted)?,
                    |count, step| count.checked_add(step.len()).ok_or_else(exhausted),
                )?;
                (0, nodes)
            }
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
            Node::CallTarget(CallTargetSnapshot::Scoped { metadata, .. }) => {
                self.charge_bytes(
                    usize::try_from(metadata.retained_bytes()).map_err(|_| exhausted())?,
                )?;
            }
            Node::CallTarget(CallTargetSnapshot::AgentRevision(reference)) => {
                self.charge_bytes(
                    usize::try_from(reference.retained_bytes()).map_err(|_| exhausted())?,
                )?;
            }
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
            Node::Host(Host::String(value) | Host::Int(value) | Host::UInt(value)) => {
                self.charge_bytes(value.len())?
            }
            Node::Host(Host::Bytes(value)) => self.charge_bytes(value.len())?,
            Node::Host(Host::Variant { name, .. }) => self.charge_bytes(name.len())?,
            Node::Host(Host::Record(fields)) => {
                for (name, _) in fields {
                    self.charge_bytes(name.len())?;
                }
            }
            Node::CallTarget(CallTargetSnapshot::StdIntrinsic { substitutions, .. }) => {
                for (name, _) in substitutions.iter().flatten() {
                    self.charge_bytes(name.len())?;
                }
            }
            Node::Model(Part::Content(ModelContentValue::Text(text))) => {
                self.charge_bytes(text.len())?
            }
            Node::Model(Part::ToolCall(call)) => {
                self.charge_bytes(call.id.len())?;
                self.charge_bytes(call.tool.len())?;
            }
            _ => {}
        }
        Ok(())
    }

    fn effect_rows(
        &mut self,
        rows: &[(String, etas_types::EffectRowRef)],
    ) -> Result<(), InterpreterCodecError> {
        self.metadata(rows.len(), 0)?;
        for (name, row) in rows {
            self.metadata(row.effects.len(), name.len())?;
            if let Some(tail) = &row.tail {
                self.metadata(1, tail.len())?;
            }
            for effect in &row.effects {
                self.metadata(effect.args.len(), effect.name.len())?;
                for arg in &effect.args {
                    match arg {
                        etas_types::EffectArgRef::String(text)
                        | etas_types::EffectArgRef::Int(text) => self.charge_bytes(text.len())?,
                        etas_types::EffectArgRef::Path(path) => {
                            self.metadata(path.len(), 0)?;
                            for segment in path {
                                self.charge_bytes(segment.len())?;
                            }
                        }
                        etas_types::EffectArgRef::Type(_) | etas_types::EffectArgRef::Wildcard => {}
                    }
                }
            }
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

fn model_work(part: Part<'_>) -> Result<(usize, usize), InterpreterCodecError> {
    Ok(match part {
        Part::Response(value) => (
            value
                .tool_calls
                .len()
                .checked_add(1)
                .ok_or_else(exhausted)?,
            usize::from(value.usage.is_some()) * 2,
        ),
        Part::Message(value) => (value.content.len(), 0),
        Part::Content(ModelContentValue::Text(_)) => (0, 0),
        Part::Content(ModelContentValue::Value(_)) | Part::ToolCall(_) => (1, 0),
    })
}

fn exhausted() -> InterpreterCodecError {
    InterpreterCodecError::new("checkpoint snapshot expansion exceeds node budget")
}

use super::budget::{MessageView, Payload, ViewBudget};
use crate::{
    orchestration::ValueSnapshot,
    value::{HostJsonSupportValue, InterpValue},
};

impl Payload for InterpValue {
    fn measure(&self, budget: &mut ViewBudget<'_>, depth: usize) -> Result<(), String> {
        use InterpValue::*;
        budget.node(depth)?;
        match self {
            Unit | Bool(_) | Number(_) | OptionNone => Ok(()),
            Message(message) => budget.envelope(
                MessageView {
                    id: &message.id,
                    session: message.session.as_deref(),
                    from: message.from.as_deref(),
                    to: message.to.as_deref(),
                    created_at: &message.created_at,
                    payload: message.payload.as_ref(),
                    provenance: message
                        .provenance
                        .as_ref()
                        .map(|p| (p.trace_id.as_deref(), p.source.as_deref())),
                },
                depth,
            ),
            String(value) => budget.charge(value.len(), depth),
            Bytes(value) => budget.charge(value.len(), depth),
            Json(value) => value.measure(budget, depth + 1),
            Nominal { value, .. } | Trust { value, .. } | OptionSome(value) => {
                value.measure(budget, depth + 1)
            }
            Variant { name, fields } => {
                budget.charge(name.len(), depth)?;
                for value in fields {
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            Tuple(values) => {
                for value in values {
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            Array(values) | Deque(values) | Queue(values) | Stack(values) => {
                for value in values.borrow().iter() {
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            List(values) => {
                for value in values.borrow().iter() {
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            Slice(values) => {
                for value in values.borrow().iter() {
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            Set(values) | OrderedSet(values) => {
                for value in values.borrow().iter() {
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            Map(values) | OrderedMap(values) | PriorityQueue(values) => {
                for (key, value) in values.borrow().iter() {
                    key.measure(budget, depth + 1)?;
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            Record(values) => {
                for (name, value) in values.borrow().iter() {
                    budget.charge(name.len(), depth)?;
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            _ => Err("unsupported value in conversation storage payload".into()),
        }
    }
}

impl Payload for ValueSnapshot {
    fn measure(&self, budget: &mut ViewBudget<'_>, depth: usize) -> Result<(), String> {
        use ValueSnapshot::*;
        budget.node(depth)?;
        match self {
            Unit | Bool(_) | Number(_) | OptionNone => Ok(()),
            Message(message) => budget.envelope(
                MessageView {
                    id: &message.id,
                    session: message.session.as_deref(),
                    from: message.from.as_deref(),
                    to: message.to.as_deref(),
                    created_at: &message.created_at,
                    payload: message.payload.as_ref(),
                    provenance: message
                        .provenance
                        .as_ref()
                        .map(|p| (p.trace_id.as_deref(), p.source.as_deref())),
                },
                depth,
            ),
            String(value) => budget.charge(value.len(), depth),
            Bytes(value) => budget.charge(value.len(), depth),
            Json(value) => value.measure(budget, depth + 1),
            Nominal { value, .. } | Trust { value, .. } | OptionSome(value) => {
                value.measure(budget, depth + 1)
            }
            Variant { name, fields } => {
                budget.charge(name.len(), depth)?;
                for value in fields {
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            Tuple(values) | Array(values) | List(values) | Slice(values) | Set(values)
            | Deque(values) | Queue(values) | Stack(values) | OrderedSet(values) => {
                for value in values {
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            Map(values) | OrderedMap(values) | PriorityQueue(values) => {
                for (key, value) in values {
                    key.measure(budget, depth + 1)?;
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            Record(values) => {
                for (name, value) in values {
                    budget.charge(name.len(), depth)?;
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            _ => Err("unsupported value in conversation storage payload".into()),
        }
    }
}

impl Payload for HostJsonSupportValue {
    fn measure(&self, budget: &mut ViewBudget<'_>, depth: usize) -> Result<(), String> {
        budget.node(depth)?;
        match self {
            Self::Null | Self::Bool(_) | Self::NumberBits(_) => Ok(()),
            Self::String(value) => budget.charge(value.len(), depth),
            Self::Array(values) => {
                for value in values {
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            Self::Object(values) => {
                for (key, value) in values {
                    budget.charge(key.len(), depth)?;
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
        }
    }
}

use etas_host::value::projection::{
    HostJsonProjection, HostJsonVisitor, HostScalar, HostValueProjection, HostValueVisitor,
};
use etas_host::{HostError, HostErrorCode};

use crate::value::{HostJsonSupportValue, InterpValue};

mod message;

pub(super) struct BorrowedValue<'a>(pub &'a InterpValue);

impl HostValueProjection for BorrowedValue<'_> {
    fn project<V: HostValueVisitor>(&self, visitor: V) -> Result<V::Output, HostError> {
        match self.0 {
            InterpValue::MemoryWriteIntent(_) => Err(projection_error(
                "write intents require the explicit storage/checkpoint codec",
            )),
            InterpValue::Unit => visitor.scalar(HostScalar::Unit),
            InterpValue::Bool(v) => visitor.scalar(HostScalar::Bool(*v)),
            InterpValue::Number(v) => super::numeric_to_host_value(*v)
                .ok_or_else(|| projection_error("non-finite numeric value is not host-encodable"))?
                .project(visitor),
            InterpValue::String(v) => visitor.scalar(HostScalar::String(v)),
            InterpValue::Bytes(v) => visitor.scalar(HostScalar::Bytes(v)),
            InterpValue::Json(v) => visitor.json(&BorrowedJson(v)),
            InterpValue::Nominal { value, .. } | InterpValue::Trust { value, .. } => {
                BorrowedValue(value).project(visitor)
            }
            InterpValue::Tuple(v) => visitor.list(v.iter().map(BorrowedValue)),
            InterpValue::Array(v) | InterpValue::Stack(v) => {
                visitor.list(v.borrow().iter().map(BorrowedValue))
            }
            InterpValue::List(v) => visitor.list(v.iter().map(BorrowedValue)),
            InterpValue::Deque(v) | InterpValue::Queue(v) => {
                visitor.list(v.borrow().iter().map(BorrowedValue))
            }
            InterpValue::Map(v) | InterpValue::OrderedMap(v) | InterpValue::PriorityQueue(v) => {
                visitor.map(
                    v.borrow()
                        .iter()
                        .map(|(k, v)| (BorrowedValue(k), BorrowedValue(v))),
                )
            }
            InterpValue::Record(v) => visitor.record(
                v.borrow()
                    .iter()
                    .map(|(k, v)| (k.as_str(), BorrowedValue(v))),
            ),
            InterpValue::Variant { name, fields } => {
                visitor.variant(name, fields.iter().map(BorrowedValue))
            }
            InterpValue::OptionNone => visitor.variant("None", std::iter::empty::<Self>()),
            InterpValue::OptionSome(v) => visitor.variant("Some", [BorrowedValue(v)]),
            InterpValue::Message(v) => message::message(v, visitor),
            InterpValue::Conversation(v) => message::conversation(v, visitor),
            other => Err(projection_error(format!(
                "{} is not host-encodable",
                super::interp_value_kind(other)
            ))),
        }
    }
}

struct BorrowedJson<'a>(&'a HostJsonSupportValue);

impl HostJsonProjection for BorrowedJson<'_> {
    fn project_json<V: HostJsonVisitor>(&self, visitor: V) -> Result<V::Output, HostError> {
        match self.0 {
            HostJsonSupportValue::Null => visitor.null(),
            HostJsonSupportValue::Bool(v) => visitor.boolean(*v),
            HostJsonSupportValue::NumberBits(v) => visitor.number(f64::from_bits(*v)),
            HostJsonSupportValue::String(v) => visitor.string(v),
            HostJsonSupportValue::Array(v) => visitor.array(v.iter().map(BorrowedJson)),
            HostJsonSupportValue::Object(v) => {
                visitor.object(v.iter().map(|(k, v)| (k.as_str(), BorrowedJson(v))))
            }
        }
    }
}

fn projection_error(message: impl Into<String>) -> HostError {
    HostError::new(HostErrorCode::SchemaMismatch, message)
}

#[cfg(test)]
mod tests;

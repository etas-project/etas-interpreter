use crate::value::*;

#[derive(Clone, PartialEq, Eq, Hash)]
enum Kind {
    Array,
    Stack,
    Tuple,
    List,
    Slice(usize, usize),
    Set,
    OrderedSet,
    Deque,
    Queue,
    Map,
    OrderedMap,
    PriorityQueue,
    Record,
    Some,
    Nominal(etas_types::TypeId),
    Trust(etas_types::TrustWrapper),
    Variant(StringValue),
}

/// Only meaningful while the immutable root (or entire frame) remains borrowed.
/// Never persist these addresses or use them as checkpoint wire identities.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct CaptureIdentity {
    kind: Kind,
    backing: *const (),
}

impl CaptureIdentity {
    pub(super) fn of(value: &InterpValue) -> Option<Self> {
        let (kind, backing) = match value {
            InterpValue::Array(v) => (Kind::Array, v.shared_capture_identity()?),
            InterpValue::Stack(v) => (Kind::Stack, v.shared_capture_identity()?),
            InterpValue::Tuple(v) => (Kind::Tuple, v.shared_capture_identity()?),
            InterpValue::List(v) => (Kind::List, v.shared_capture_identity()?),
            InterpValue::Slice(v) => {
                let (backing, start, end) = v.shared_capture_identity()?;
                (Kind::Slice(start, end), backing)
            }
            InterpValue::Set(v) => (Kind::Set, v.shared_capture_identity()?),
            InterpValue::OrderedSet(v) => (Kind::OrderedSet, v.shared_capture_identity()?),
            InterpValue::Deque(v) => (Kind::Deque, v.shared_capture_identity()?),
            InterpValue::Queue(v) => (Kind::Queue, v.shared_capture_identity()?),
            InterpValue::Map(v) => (Kind::Map, v.shared_capture_identity()?),
            InterpValue::OrderedMap(v) => (Kind::OrderedMap, v.shared_capture_identity()?),
            InterpValue::PriorityQueue(v) => (Kind::PriorityQueue, v.shared_capture_identity()?),
            InterpValue::Record(v) => (Kind::Record, v.shared_capture_identity()?),
            InterpValue::OptionSome(v) => (Kind::Some, v.shared_capture_identity()?),
            InterpValue::Nominal { ty, value } => {
                (Kind::Nominal(*ty), value.shared_capture_identity()?)
            }
            InterpValue::Trust { wrapper, value } => {
                (Kind::Trust(*wrapper), value.shared_capture_identity()?)
            }
            InterpValue::Variant { name, fields } => (
                Kind::Variant(name.clone()),
                fields.shared_capture_identity()?,
            ),
            // Envelope metadata is not identified by its shared payload alone.
            _ => return None,
        };
        Some(Self { kind, backing })
    }
}

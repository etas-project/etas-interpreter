use std::hash::{Hash, Hasher};

use super::{
    MembershipValue,
    structure::{StructuralPartition, hash_structure},
};
use crate::value::{
    ArrayValue, DequeValue, InterpValue, ListValue, MapValue, RecordValue, SetValue, SharedFields,
    SharedValue, SliceValue,
};

#[cfg(test)]
thread_local! { static VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(super) fn reset_visits() {
    VISITS.with(|visits| visits.set(0));
}
#[cfg(test)]
pub(super) fn visits() -> usize {
    VISITS.with(std::cell::Cell::get)
}

pub(crate) struct Cursor {
    storage: Storage,
    index: usize,
    identity: Option<crate::value::identity::ValueIdentity>,
    child_context: *const (),
}

enum Storage {
    Single(SharedValue),
    Fields(SharedFields),
    Array(ArrayValue),
    Slice(SliceValue),
    List(ListValue),
    Deque(DequeValue),
    Set(SetValue),
    Map(MapValue),
    OrderedSet(Box<crate::value::ordered::Cursor>),
    OrderedMap(Box<crate::value::ordered::Cursor>),
    PriorityQueue {
        queue: crate::value::PriorityQueueValue,
        slot: usize,
    },
    Record(RecordValue),
}

impl MembershipValue for InterpValue {
    fn hash_partition(&self, state: &mut (impl Hasher + Clone)) {
        hash_structure(self, state);
    }
}

impl StructuralPartition for InterpValue {
    type Identity = crate::value::identity::ValueIdentity;
    type Cursor<'a> = Cursor;

    fn start(&self, state: &mut impl Hasher) -> Option<Cursor> {
        #[cfg(test)]
        VISITS.with(|visits| visits.set(visits.get() + 1));
        let identity = Self::Identity::of(self);
        std::mem::discriminant(self).hash(state);
        let storage = match self {
            Self::Bool(value) => {
                value.hash(state);
                return None;
            }
            Self::Number(value) => {
                value.hash(state);
                return None;
            }
            Self::StdConstant(value) => {
                (value.role() as u8).hash(state);
                value.ty().hash(state);
                return None;
            }
            Self::String(value) => {
                value.hash_membership(state);
                return None;
            }
            Self::Bytes(value) => {
                value.hash(state);
                return None;
            }
            Self::Json(value) => {
                crate::value::json::hash(value, state);
                return None;
            }
            Self::Range(value) => {
                std::mem::discriminant(&value.bounds).hash(state);
                // Checked ranges have numeric endpoints; hashing never iterates
                // the represented interval or substitutes iteration equivalence.
                value.start.as_number().hash(state);
                value.end.as_number().hash(state);
                return None;
            }
            Self::Nominal { ty, value } => {
                ty.hash(state);
                Storage::Single(value.clone())
            }
            Self::Trust { wrapper, value } => {
                wrapper.hash(state);
                Storage::Single(value.clone())
            }
            Self::OptionSome(value) => Storage::Single(value.clone()),
            Self::Tuple(values) => {
                values.len().hash(state);
                Storage::Fields(values.clone())
            }
            Self::Variant { name, fields } => {
                name.hash_membership(state);
                fields.len().hash(state);
                Storage::Fields(fields.clone())
            }
            Self::Array(values) | Self::Stack(values) => {
                values.borrow().len().hash(state);
                Storage::Array(values.clone())
            }
            Self::Slice(values) => {
                values.borrow().len().hash(state);
                Storage::Slice(values.clone())
            }
            Self::List(values) => {
                values.len().hash(state);
                Storage::List(values.clone())
            }
            Self::Deque(values) | Self::Queue(values) => {
                values.borrow().len().hash(state);
                Storage::Deque(values.clone())
            }
            Self::Set(values) => {
                values.borrow().len().hash(state);
                Storage::Set(values.clone())
            }
            Self::OrderedSet(values) => {
                values.len().hash(state);
                Storage::OrderedSet(Box::new(values.cursor()))
            }
            Self::PriorityQueue(values) => {
                values.len().hash(state);
                Storage::PriorityQueue {
                    queue: values.clone(),
                    slot: 0,
                }
            }
            Self::Map(values) => {
                values.borrow().len().hash(state);
                Storage::Map(values.clone())
            }
            Self::OrderedMap(values) => {
                values.len().hash(state);
                Storage::OrderedMap(Box::new(values.cursor()))
            }
            Self::Record(values) => {
                values.borrow().len().hash(state);
                Storage::Record(values.clone())
            }
            // Opaque runtime objects remain equality-checked within their kind.
            // A partition is not permission to erase or serialize their identity.
            _ => return None,
        };
        Some(Cursor {
            storage,
            index: 0,
            identity,
            child_context: std::ptr::null(),
        })
    }

    fn identity(cursor: &Cursor) -> Option<Self::Identity> {
        cursor.identity.clone()
    }

    fn child_context(cursor: &Cursor) -> *const () {
        cursor.child_context
    }

    fn next<'a>(cursor: &mut Cursor, state: &mut impl Hasher) -> Option<Option<Cursor>>
    where
        Self: 'a,
    {
        let index = cursor.index;
        cursor.index += 1;
        cursor.child_context = std::ptr::null();
        Some(match &mut cursor.storage {
            Storage::Single(value) => {
                if index != 0 {
                    return None;
                }
                value.start(state)
            }
            Storage::Fields(values) => values.get(index)?.start(state),
            Storage::Array(values) => values.borrow().get(index)?.start(state),
            Storage::Slice(values) => values.borrow().get(index)?.start(state),
            Storage::Deque(values) => values.borrow().get(index)?.start(state),
            Storage::Set(values) => values.borrow().get(index)?.start(state),
            Storage::OrderedSet(cursor) => {
                let child = cursor.current()?.0.start(state);
                cursor.advance();
                child
            }
            Storage::List(values) => {
                let child = values.get(0)?.start(state);
                values.advance();
                child
            }
            Storage::PriorityQueue { queue, slot } => {
                while *slot < queue.slot_count() && queue.slot(*slot).is_none() {
                    *slot += 1;
                }
                let pair = queue.slot(*slot)?;
                if index % 2 == 0 {
                    pair.0.start(state)
                } else {
                    *slot += 1;
                    pair.1.start(state)
                }
            }
            Storage::Map(values) => {
                let values = values.borrow();
                let pair = values.get(index / 2)?;
                if index % 2 == 0 {
                    pair.0.start(state)
                } else {
                    pair.1.start(state)
                }
            }
            Storage::OrderedMap(cursor) => {
                let pair = cursor.current()?;
                if index % 2 == 0 {
                    pair.0.start(state)
                } else {
                    let child = pair.1.start(state);
                    cursor.advance();
                    child
                }
            }
            Storage::Record(values) => {
                let values = values.borrow();
                let (name, value) = values.get(index)?;
                cursor.child_context = (name as *const String).cast();
                name.hash(state);
                value.start(state)
            }
        })
    }

    fn unordered(cursor: &Cursor) -> bool {
        matches!(cursor.storage, Storage::Set(_) | Storage::Record(_))
    }
}

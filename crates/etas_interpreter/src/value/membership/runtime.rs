use std::hash::{Hash, Hasher};

use super::{
    MembershipValue,
    structure::{StructuralPartition, hash_structure},
};
use crate::value::{
    ArrayValue, DequeValue, InterpValue, ListValue, MapValue, RecordValue, SetValue, SharedFields,
    SharedValue, SliceValue,
};

pub(crate) struct Cursor {
    storage: Storage,
    index: usize,
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
    Record(RecordValue),
}

impl MembershipValue for InterpValue {
    fn hash_partition(&self, state: &mut (impl Hasher + Clone)) {
        hash_structure(self, state);
    }
}

impl StructuralPartition for InterpValue {
    type Cursor<'a> = Cursor;

    fn start(&self, state: &mut impl Hasher) -> Option<Cursor> {
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
            Self::String(value) => {
                value.hash(state);
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
                name.hash(state);
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
            Self::Set(values) | Self::OrderedSet(values) => {
                values.borrow().len().hash(state);
                Storage::Set(values.clone())
            }
            Self::Map(values) | Self::OrderedMap(values) | Self::PriorityQueue(values) => {
                values.borrow().len().hash(state);
                Storage::Map(values.clone())
            }
            Self::Record(values) => {
                values.borrow().len().hash(state);
                Storage::Record(values.clone())
            }
            // Opaque runtime objects remain equality-checked within their kind.
            // A partition is not permission to erase or serialize their identity.
            _ => return None,
        };
        Some(Cursor { storage, index: 0 })
    }

    fn next<'a>(cursor: &mut Cursor, state: &mut impl Hasher) -> Option<Option<Cursor>>
    where
        Self: 'a,
    {
        let index = cursor.index;
        cursor.index += 1;
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
            Storage::List(values) => {
                let child = values.get(0)?.start(state);
                values.advance();
                child
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
            Storage::Record(values) => {
                let values = values.borrow();
                let (name, value) = values.get(index)?;
                name.hash(state);
                value.start(state)
            }
        })
    }

    fn unordered(cursor: &Cursor) -> bool {
        matches!(cursor.storage, Storage::Set(_))
    }
}

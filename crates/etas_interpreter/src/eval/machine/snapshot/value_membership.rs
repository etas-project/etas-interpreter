use std::hash::{Hash, Hasher};

use crate::{
    orchestration::ValueSnapshot,
    value::membership::{
        MembershipValue,
        structure::{StructuralPartition, hash_structure},
    },
};

pub(crate) struct Cursor<'a> {
    storage: Storage<'a>,
    index: usize,
}

enum Storage<'a> {
    Single(&'a ValueSnapshot),
    Sequence(&'a [ValueSnapshot]),
    Set(&'a [ValueSnapshot]),
    Map(&'a [(ValueSnapshot, ValueSnapshot)]),
    Record(&'a [(String, ValueSnapshot)]),
}

impl MembershipValue for ValueSnapshot {
    fn member_eq(&self, other: &Self) -> bool {
        super::value_compare::membership_equal(self, other)
    }

    fn hash_partition(&self, state: &mut (impl Hasher + Clone)) {
        hash_structure(self, state);
    }
}

impl StructuralPartition for ValueSnapshot {
    type Cursor<'a> = Cursor<'a>;

    fn start(&self, state: &mut impl Hasher) -> Option<Cursor<'_>> {
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
                Storage::Single(value)
            }
            Self::Trust { wrapper, value } => {
                wrapper.hash(state);
                Storage::Single(value)
            }
            Self::OptionSome(value) => Storage::Single(value),
            Self::Tuple(values)
            | Self::Array(values)
            | Self::List(values)
            | Self::Slice(values)
            | Self::Stack(values)
            | Self::Deque(values)
            | Self::Queue(values) => {
                values.len().hash(state);
                Storage::Sequence(values)
            }
            Self::Variant { name, fields } => {
                name.hash(state);
                fields.len().hash(state);
                Storage::Sequence(fields)
            }
            Self::Set(values) | Self::OrderedSet(values) => {
                values.len().hash(state);
                Storage::Set(values)
            }
            Self::Map(values) | Self::OrderedMap(values) | Self::PriorityQueue(values) => {
                values.len().hash(state);
                Storage::Map(values)
            }
            Self::Record(values) => {
                values.len().hash(state);
                Storage::Record(values)
            }
            _ => return None,
        };
        Some(Cursor { storage, index: 0 })
    }

    fn next<'a>(cursor: &mut Cursor<'a>, state: &mut impl Hasher) -> Option<Option<Cursor<'a>>>
    where
        Self: 'a,
    {
        let index = cursor.index;
        cursor.index += 1;
        Some(match &cursor.storage {
            Storage::Single(value) => {
                if index != 0 {
                    return None;
                }
                value.start(state)
            }
            Storage::Sequence(values) | Storage::Set(values) => values.get(index)?.start(state),
            Storage::Map(values) => {
                let pair = values.get(index / 2)?;
                if index % 2 == 0 {
                    pair.0.start(state)
                } else {
                    pair.1.start(state)
                }
            }
            Storage::Record(values) => {
                let (name, value) = values.get(index)?;
                name.hash(state);
                value.start(state)
            }
        })
    }

    fn unordered(cursor: &Cursor<'_>) -> bool {
        matches!(cursor.storage, Storage::Set(_))
    }
}

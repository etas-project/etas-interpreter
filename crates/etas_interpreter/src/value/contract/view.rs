use crate::{
    control::CallTarget,
    orchestration::{CallTargetSnapshot, ValueSnapshot},
    value::{InterpValue, NumericValue, StringValue},
};
use etas_std::RuntimeTypeRole;
use etas_types::{TrustWrapper, TypeId};

#[derive(Clone, Copy)]
pub(crate) enum Value<'a> {
    Runtime(&'a InterpValue),
    Snapshot(&'a ValueSnapshot),
}
#[derive(Clone, Copy)]
pub(crate) enum Target<'a> {
    Runtime(&'a CallTarget),
    Snapshot(&'a CallTargetSnapshot),
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SequenceKind {
    Array,
    List,
    Slice,
    Set,
    Tuple,
    Native(RuntimeTypeRole),
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MapKind {
    Map,
    Native(RuntimeTypeRole),
}
pub(super) type Values<'a> = Box<dyn Iterator<Item = Value<'a>> + 'a>;
pub(super) type Pairs<'a> = Box<dyn Iterator<Item = (Value<'a>, Value<'a>)> + 'a>;
pub(super) type Fields<'a> = Box<dyn Iterator<Item = (&'a str, Value<'a>)> + 'a>;
pub(super) enum Node<'a> {
    Unit,
    Bool,
    Number(NumericValue),
    String(&'a StringValue),
    Bytes,
    Nominal(TypeId, Value<'a>),
    Trust(TrustWrapper, Value<'a>),
    None,
    Some(Value<'a>),
    Sequence(SequenceKind, usize, Values<'a>),
    Map(MapKind, Pairs<'a>),
    Record(usize, Fields<'a>),
    Variant(&'a str, usize, Values<'a>),
    Range(Value<'a>, Value<'a>),
    Native,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum Identity<'a> {
    Runtime {
        kind: u8,
        pointer: usize,
        start: usize,
        end: usize,
        ty: TypeId,
        name: &'a str,
    },
    Snapshot {
        kind: u8,
        pointer: usize,
        len: usize,
        ty: TypeId,
        name: &'a str,
    },
}

impl<'a> Value<'a> {
    pub(super) fn identity(self) -> Option<Identity<'a>> {
        match self {
            Self::Runtime(value) => {
                use InterpValue as R;
                let mut start = 0;
                let mut end = 0;
                let (kind, pointer, ty, name) = match value {
                    R::Nominal { ty, value } => (0, value.shared_capture_identity()?, *ty, ""),
                    R::Trust { wrapper, value } => {
                        start = *wrapper as usize;
                        (1, value.shared_capture_identity()?, TypeId(0), "")
                    }
                    R::OptionSome(value) => (2, value.shared_capture_identity()?, TypeId(0), ""),
                    R::Array(value) => (3, value.shared_capture_identity()?, TypeId(0), ""),
                    R::List(value) => (4, value.shared_capture_identity()?, TypeId(0), ""),
                    R::Slice(value) => {
                        let (pointer, from, to) = value.shared_capture_identity()?;
                        start = from;
                        end = to;
                        (5, pointer, TypeId(0), "")
                    }
                    R::Set(value) => (6, value.shared_capture_identity()?, TypeId(0), ""),
                    R::Tuple(value) => (7, value.shared_capture_identity()?, TypeId(0), ""),
                    R::Record(value) => (8, value.shared_capture_identity()?, TypeId(0), ""),
                    R::Variant { name, fields } => (
                        9,
                        fields.shared_capture_identity()?,
                        TypeId(0),
                        name.as_str(),
                    ),
                    R::Map(value) => (10, value.shared_capture_identity()?, TypeId(0), ""),
                    R::Deque(value) => (11, value.shared_capture_identity()?, TypeId(0), ""),
                    R::Queue(value) => (12, value.shared_capture_identity()?, TypeId(0), ""),
                    R::Stack(value) => (13, value.shared_capture_identity()?, TypeId(0), ""),
                    R::OrderedMap(value) => (14, value.shared_capture_identity()?, TypeId(0), ""),
                    R::OrderedSet(value) => (15, value.shared_capture_identity()?, TypeId(0), ""),
                    R::PriorityQueue(value) => {
                        (16, value.shared_capture_identity()?, TypeId(0), "")
                    }
                    _ => return None,
                };
                Some(Identity::Runtime {
                    kind,
                    pointer: pointer as usize,
                    start,
                    end,
                    ty,
                    name,
                })
            }
            Self::Snapshot(value) => {
                use ValueSnapshot as S;
                let (kind, pointer, len, ty, name) = match value {
                    S::Nominal { ty, value } => {
                        (0, value.as_ref() as *const S as usize, 1, *ty, "")
                    }
                    S::Trust { wrapper, value } => (
                        1,
                        value.as_ref() as *const S as usize,
                        *wrapper as usize,
                        TypeId(0),
                        "",
                    ),
                    S::OptionSome(value) => {
                        (2, value.as_ref() as *const S as usize, 1, TypeId(0), "")
                    }
                    S::Array(v) => (3, v.as_ptr() as usize, v.len(), TypeId(0), ""),
                    S::List(v) => (4, v.as_ptr() as usize, v.len(), TypeId(0), ""),
                    S::Slice(v) => (5, v.as_ptr() as usize, v.len(), TypeId(0), ""),
                    S::Set(v) => (6, v.as_ptr() as usize, v.len(), TypeId(0), ""),
                    S::Tuple(v) => (7, v.as_ptr() as usize, v.len(), TypeId(0), ""),
                    S::Record(v) => (8, v.as_ptr() as usize, v.len(), TypeId(0), ""),
                    S::Variant { name, fields } => (
                        9,
                        fields.as_ptr() as usize,
                        fields.len(),
                        TypeId(0),
                        name.as_str(),
                    ),
                    S::Map(v) => (10, v.as_ptr() as usize, v.len(), TypeId(0), ""),
                    S::Deque(v) => (11, v.as_ptr() as usize, v.len(), TypeId(0), ""),
                    S::Queue(v) => (12, v.as_ptr() as usize, v.len(), TypeId(0), ""),
                    S::Stack(v) => (13, v.as_ptr() as usize, v.len(), TypeId(0), ""),
                    S::OrderedMap { entries, .. } => {
                        (14, entries.as_ptr() as usize, entries.len(), TypeId(0), "")
                    }
                    S::OrderedSet { values, .. } => {
                        (15, values.as_ptr() as usize, values.len(), TypeId(0), "")
                    }
                    S::PriorityQueue { entries, .. } => {
                        (16, entries.as_ptr() as usize, entries.len(), TypeId(0), "")
                    }
                    _ => return None,
                };
                Some(Identity::Snapshot {
                    kind,
                    pointer,
                    len,
                    ty,
                    name,
                })
            }
        }
    }

    pub(super) fn comparator(self) -> Result<Option<Target<'a>>, String> {
        Ok(match self {
            Self::Runtime(InterpValue::PriorityQueue(value)) => {
                value.comparator().map(Target::Runtime)
            }
            Self::Runtime(InterpValue::OrderedMap(value)) => {
                value.comparator().map(Target::Runtime)
            }
            Self::Runtime(InterpValue::OrderedSet(value)) => {
                value.comparator().map(Target::Runtime)
            }
            Self::Snapshot(
                ValueSnapshot::PriorityQueue {
                    comparator: Some(value),
                    ..
                }
                | ValueSnapshot::OrderedMap {
                    comparator: Some(value),
                    ..
                }
                | ValueSnapshot::OrderedSet {
                    comparator: Some(value),
                    ..
                },
            ) => match value.as_ref() {
                ValueSnapshot::Callable(target) => Some(Target::Snapshot(target)),
                _ => return Err("collection comparator is not a callable".into()),
            },
            _ => None,
        })
    }

    pub(super) fn node(self) -> Node<'a> {
        use InterpValue as R;
        use RuntimeTypeRole as Role;
        use ValueSnapshot as S;
        match self {
            Self::Runtime(value) => match value {
                R::Unit => Node::Unit,
                R::Bool(_) => Node::Bool,
                R::Number(value) => Node::Number(*value),
                R::String(value) => Node::String(value),
                R::Bytes(_) => Node::Bytes,
                R::Nominal { ty, value } => Node::Nominal(*ty, Self::Runtime(value)),
                R::Trust { wrapper, value } => Node::Trust(*wrapper, Self::Runtime(value)),
                R::OptionNone => Node::None,
                R::OptionSome(value) => Node::Some(Self::Runtime(value)),
                R::Array(v) => Node::Sequence(
                    SequenceKind::Array,
                    v.borrow().len(),
                    Box::new(v.borrow().iter().map(Self::Runtime)),
                ),
                R::List(v) => Node::Sequence(
                    SequenceKind::List,
                    v.len(),
                    Box::new(v.iter().map(Self::Runtime)),
                ),
                R::Slice(v) => Node::Sequence(
                    SequenceKind::Slice,
                    v.borrow().len(),
                    Box::new(v.borrow().iter().map(Self::Runtime)),
                ),
                R::Set(v) => Node::Sequence(
                    SequenceKind::Set,
                    v.borrow().len(),
                    Box::new(v.borrow().iter().map(Self::Runtime)),
                ),
                R::Tuple(v) => Node::Sequence(
                    SequenceKind::Tuple,
                    v.len(),
                    Box::new(v.iter().map(Self::Runtime)),
                ),
                R::Deque(v) => Node::Sequence(
                    SequenceKind::Native(Role::Deque),
                    v.borrow().len(),
                    Box::new(v.borrow().iter().map(Self::Runtime)),
                ),
                R::Queue(v) => Node::Sequence(
                    SequenceKind::Native(Role::Queue),
                    v.borrow().len(),
                    Box::new(v.borrow().iter().map(Self::Runtime)),
                ),
                R::Stack(v) => Node::Sequence(
                    SequenceKind::Native(Role::Stack),
                    v.borrow().len(),
                    Box::new(v.borrow().iter().map(Self::Runtime)),
                ),
                R::OrderedSet(v) => Node::Sequence(
                    SequenceKind::Native(Role::OrderedSet),
                    v.len(),
                    Box::new(v.iter().map(|(key, _)| Self::Runtime(key))),
                ),
                R::Map(v) => Node::Map(
                    MapKind::Map,
                    Box::new(
                        v.borrow()
                            .iter()
                            .map(|(a, b)| (Self::Runtime(a), Self::Runtime(b))),
                    ),
                ),
                R::OrderedMap(v) => Node::Map(
                    MapKind::Native(Role::OrderedMap),
                    Box::new(v.iter().map(|(a, b)| (Self::Runtime(a), Self::Runtime(b)))),
                ),
                R::PriorityQueue(v) => Node::Map(
                    MapKind::Native(Role::PriorityQueue),
                    Box::new(v.iter().map(|(a, b)| (Self::Runtime(a), Self::Runtime(b)))),
                ),
                R::Record(v) => Node::Record(
                    v.borrow().len(),
                    Box::new(
                        v.borrow()
                            .iter()
                            .map(|(name, value)| (name.as_str(), Self::Runtime(value))),
                    ),
                ),
                R::Variant { name, fields } => Node::Variant(
                    name.as_str(),
                    fields.len(),
                    Box::new(fields.iter().map(Self::Runtime)),
                ),
                R::Range(v) => Node::Range(Self::Runtime(&v.start), Self::Runtime(&v.end)),
                _ => Node::Native,
            },
            Self::Snapshot(value) => match value {
                S::Unit => Node::Unit,
                S::Bool(_) => Node::Bool,
                S::Number(value) => Node::Number(*value),
                S::String(value) => Node::String(value),
                S::Bytes(_) => Node::Bytes,
                S::Nominal { ty, value } => Node::Nominal(*ty, Self::Snapshot(value)),
                S::Trust { wrapper, value } => Node::Trust(*wrapper, Self::Snapshot(value)),
                S::OptionNone => Node::None,
                S::OptionSome(value) => Node::Some(Self::Snapshot(value)),
                S::Array(v) => Node::Sequence(
                    SequenceKind::Array,
                    v.len(),
                    Box::new(v.iter().map(Self::Snapshot)),
                ),
                S::List(v) => Node::Sequence(
                    SequenceKind::List,
                    v.len(),
                    Box::new(v.iter().map(Self::Snapshot)),
                ),
                S::Slice(v) => Node::Sequence(
                    SequenceKind::Slice,
                    v.len(),
                    Box::new(v.iter().map(Self::Snapshot)),
                ),
                S::Set(v) => Node::Sequence(
                    SequenceKind::Set,
                    v.len(),
                    Box::new(v.iter().map(Self::Snapshot)),
                ),
                S::Tuple(v) => Node::Sequence(
                    SequenceKind::Tuple,
                    v.len(),
                    Box::new(v.iter().map(Self::Snapshot)),
                ),
                S::Deque(v) => Node::Sequence(
                    SequenceKind::Native(Role::Deque),
                    v.len(),
                    Box::new(v.iter().map(Self::Snapshot)),
                ),
                S::Queue(v) => Node::Sequence(
                    SequenceKind::Native(Role::Queue),
                    v.len(),
                    Box::new(v.iter().map(Self::Snapshot)),
                ),
                S::Stack(v) => Node::Sequence(
                    SequenceKind::Native(Role::Stack),
                    v.len(),
                    Box::new(v.iter().map(Self::Snapshot)),
                ),
                S::OrderedSet { values, .. } => Node::Sequence(
                    SequenceKind::Native(Role::OrderedSet),
                    values.len(),
                    Box::new(values.iter().map(Self::Snapshot)),
                ),
                S::Map(v) => Node::Map(
                    MapKind::Map,
                    Box::new(
                        v.iter()
                            .map(|(a, b)| (Self::Snapshot(a), Self::Snapshot(b))),
                    ),
                ),
                S::OrderedMap { entries, .. } => Node::Map(
                    MapKind::Native(Role::OrderedMap),
                    Box::new(
                        entries
                            .iter()
                            .map(|(a, b)| (Self::Snapshot(a), Self::Snapshot(b))),
                    ),
                ),
                S::PriorityQueue { entries, .. } => Node::Map(
                    MapKind::Native(Role::PriorityQueue),
                    Box::new(
                        entries
                            .iter()
                            .map(|(a, b)| (Self::Snapshot(a), Self::Snapshot(b))),
                    ),
                ),
                S::Record(v) => Node::Record(
                    v.len(),
                    Box::new(
                        v.iter()
                            .map(|(name, value)| (name.as_str(), Self::Snapshot(value))),
                    ),
                ),
                S::Variant { name, fields } => Node::Variant(
                    name.as_str(),
                    fields.len(),
                    Box::new(fields.iter().map(Self::Snapshot)),
                ),
                S::Range { start, end, .. } => {
                    Node::Range(Self::Snapshot(start), Self::Snapshot(end))
                }
                _ => Node::Native,
            },
        }
    }
}

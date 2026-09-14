use super::RestoreContext;
use crate::{
    orchestration::{SnapshotChildren, ValueSnapshot},
    value::*,
};

enum Started {
    Value(InterpValue),
    Pending(Frame),
}
enum UnaryKind {
    Nominal(etas_types::TypeId),
    Trust(etas_types::TrustWrapper),
    Some,
}
enum SequenceKind {
    Tuple,
    Array,
    List,
    Slice,
    Set,
    Deque,
    Queue,
    Stack,
    OrderedSet,
    Variant(String),
}
enum PairKind {
    Map,
    OrderedMap,
    PriorityQueue,
}
enum UnaryState {
    Child(ValueSnapshot),
    Waiting,
    Complete(InterpValue),
}
enum Frame {
    Unary {
        kind: UnaryKind,
        state: UnaryState,
    },
    Sequence {
        kind: SequenceKind,
        remaining: std::vec::IntoIter<ValueSnapshot>,
        values: Vec<InterpValue>,
    },
    Pairs {
        kind: PairKind,
        remaining: std::vec::IntoIter<(ValueSnapshot, ValueSnapshot)>,
        pending_value: Option<ValueSnapshot>,
        awaiting_value: bool,
        values: Vec<(InterpValue, InterpValue)>,
    },
    Record {
        remaining: std::vec::IntoIter<(String, ValueSnapshot)>,
        name: Option<String>,
        values: Vec<(String, InterpValue)>,
    },
}

pub(super) fn restore(
    value: ValueSnapshot,
    context: &mut RestoreContext,
) -> Result<InterpValue, String> {
    let mut frame = match start(value, context)? {
        Started::Value(value) => return Ok(value),
        Started::Pending(frame) => frame,
    };
    let mut parents = Vec::new();
    loop {
        if let Some(child) = frame.next() {
            match start(child, context)? {
                Started::Value(value) => frame.accept(value)?,
                Started::Pending(child) => {
                    parents.push(frame);
                    frame = child;
                }
            }
        } else {
            let value = frame.finish()?;
            let Some(mut parent) = parents.pop() else {
                return Ok(value);
            };
            parent.accept(value)?;
            frame = parent;
        }
    }
}

fn start(value: ValueSnapshot, context: &mut RestoreContext) -> Result<Started, String> {
    let frame = match value {
        ValueSnapshot::Nominal { ty, value } => unary(UnaryKind::Nominal(ty), value.into_value()),
        ValueSnapshot::Trust { wrapper, value } => {
            unary(UnaryKind::Trust(wrapper), value.into_value())
        }
        ValueSnapshot::OptionSome(value) => unary(UnaryKind::Some, value.into_value()),
        ValueSnapshot::Tuple(v) => sequence(SequenceKind::Tuple, v),
        ValueSnapshot::Array(v) => sequence(SequenceKind::Array, v),
        ValueSnapshot::List(v) => sequence(SequenceKind::List, v),
        ValueSnapshot::Slice(v) => sequence(SequenceKind::Slice, v),
        ValueSnapshot::Set(v) => sequence(SequenceKind::Set, v),
        ValueSnapshot::Deque(v) => sequence(SequenceKind::Deque, v),
        ValueSnapshot::Queue(v) => sequence(SequenceKind::Queue, v),
        ValueSnapshot::Stack(v) => sequence(SequenceKind::Stack, v),
        ValueSnapshot::OrderedSet(v) => sequence(SequenceKind::OrderedSet, v),
        ValueSnapshot::Variant { name, fields } => sequence(SequenceKind::Variant(name), fields),
        ValueSnapshot::Map(v) => pairs(PairKind::Map, v),
        ValueSnapshot::OrderedMap(v) => pairs(PairKind::OrderedMap, v),
        ValueSnapshot::PriorityQueue(v) => pairs(PairKind::PriorityQueue, v),
        ValueSnapshot::Record(v) => Frame::Record {
            values: Vec::with_capacity(v.len()),
            remaining: v.into_iter(),
            name: None,
        },
        other => return other.restore_leaf_with(context).map(Started::Value),
    };
    Ok(Started::Pending(frame))
}

fn unary(kind: UnaryKind, value: ValueSnapshot) -> Frame {
    Frame::Unary {
        kind,
        state: UnaryState::Child(value),
    }
}
fn sequence(kind: SequenceKind, values: SnapshotChildren<ValueSnapshot>) -> Frame {
    Frame::Sequence {
        kind,
        values: Vec::with_capacity(values.len()),
        remaining: values.into_iter(),
    }
}
fn pairs(kind: PairKind, values: SnapshotChildren<(ValueSnapshot, ValueSnapshot)>) -> Frame {
    Frame::Pairs {
        kind,
        values: Vec::with_capacity(values.len()),
        remaining: values.into_iter(),
        pending_value: None,
        awaiting_value: false,
    }
}

impl Frame {
    fn next(&mut self) -> Option<ValueSnapshot> {
        match self {
            Self::Unary { state, .. } => match std::mem::replace(state, UnaryState::Waiting) {
                UnaryState::Child(child) => Some(child),
                other => {
                    *state = other;
                    None
                }
            },
            Self::Sequence { remaining, .. } => remaining.next(),
            Self::Pairs {
                remaining,
                pending_value,
                ..
            } => {
                if let Some(value) = pending_value.take() {
                    return Some(value);
                }
                let (key, value) = remaining.next()?;
                *pending_value = Some(value);
                Some(key)
            }
            Self::Record {
                remaining, name, ..
            } => {
                let (field, value) = remaining.next()?;
                *name = Some(field);
                Some(value)
            }
        }
    }
    fn accept(&mut self, value: InterpValue) -> Result<(), String> {
        match self {
            Self::Unary { state, .. } => {
                if !matches!(state, UnaryState::Waiting) {
                    return Err("restored unary value is not awaiting its payload".into());
                }
                *state = UnaryState::Complete(value);
            }
            Self::Sequence { values, .. } => values.push(value),
            Self::Pairs {
                awaiting_value,
                values,
                ..
            } => {
                if *awaiting_value {
                    values.last_mut().ok_or("restored map value has no key")?.1 = value;
                    *awaiting_value = false;
                } else {
                    // Only a complete pair may leave this builder.
                    values.push((value, InterpValue::Unit));
                    *awaiting_value = true;
                }
            }
            Self::Record { name, values, .. } => {
                values.push((name.take().ok_or("restored field has no name")?, value))
            }
        }
        Ok(())
    }
    fn finish(self) -> Result<InterpValue, String> {
        Ok(match self {
            Self::Unary { kind, state } => {
                let UnaryState::Complete(value) = state else {
                    return Err("restored unary value has no payload".into());
                };
                let value = SharedValue::new(value);
                match kind {
                    UnaryKind::Nominal(ty) => InterpValue::Nominal { ty, value },
                    UnaryKind::Trust(wrapper) => InterpValue::Trust { wrapper, value },
                    UnaryKind::Some => InterpValue::OptionSome(value),
                }
            }
            Self::Sequence { kind, values, .. } => match kind {
                SequenceKind::Tuple => InterpValue::Tuple(values.into()),
                SequenceKind::Array => InterpValue::Array(ArrayValue::new(values)),
                SequenceKind::List => InterpValue::List(ListValue::new(values)),
                SequenceKind::Slice => InterpValue::Slice(SliceValue::new(values)),
                SequenceKind::Set => InterpValue::Set(SetValue::new(values)),
                SequenceKind::Deque => InterpValue::Deque(values.into()),
                SequenceKind::Queue => InterpValue::Queue(values.into()),
                SequenceKind::Stack => InterpValue::Stack(ArrayValue::new(values)),
                SequenceKind::OrderedSet => InterpValue::OrderedSet(SetValue::new(values)),
                SequenceKind::Variant(name) => InterpValue::Variant {
                    name: name.into(),
                    fields: values.into(),
                },
            },
            Self::Pairs {
                kind,
                awaiting_value,
                pending_value,
                values,
                ..
            } => {
                if awaiting_value || pending_value.is_some() {
                    return Err("restored map has an unmatched key or value".into());
                }
                let values = MapValue::new(values);
                match kind {
                    PairKind::Map => InterpValue::Map(values),
                    PairKind::OrderedMap => InterpValue::OrderedMap(values),
                    PairKind::PriorityQueue => InterpValue::PriorityQueue(values),
                }
            }
            Self::Record { name, values, .. } => {
                if name.is_some() {
                    return Err("restored record has an unmatched field name".into());
                }
                InterpValue::Record(RecordValue::new(values))
            }
        })
    }
}

use crate::{
    orchestration::{SnapshotBox, ValueSnapshot},
    value::*,
};

enum Started {
    Value(ValueSnapshot),
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

enum SequenceSource {
    Fields(SharedFields),
    Array(ArrayValue),
    List(ListValue),
    Slice(SliceValue),
    Set(SetValue),
    Deque(DequeValue),
}

enum Frame {
    Unary {
        kind: UnaryKind,
        source: SharedValue,
        value: Option<ValueSnapshot>,
    },
    Sequence {
        kind: SequenceKind,
        source: SequenceSource,
        values: Vec<ValueSnapshot>,
    },
    Pairs {
        kind: PairKind,
        source: MapValue,
        key: Option<ValueSnapshot>,
        values: Vec<(ValueSnapshot, ValueSnapshot)>,
    },
    Record {
        source: RecordValue,
        values: Vec<(String, ValueSnapshot)>,
    },
}

pub(super) fn capture(value: &InterpValue) -> Result<ValueSnapshot, String> {
    let mut frame = match start(value)? {
        Started::Value(value) => return Ok(value),
        Started::Pending(frame) => frame,
    };
    // Flat containers need no traversal allocation. Only nested aggregates
    // spill their suspended builders onto the explicit ancestor stack.
    let mut parents = Vec::new();
    loop {
        match frame.next()? {
            Some(Started::Value(value)) => frame.accept(value)?,
            Some(Started::Pending(child)) => {
                parents.push(frame);
                frame = child;
            }
            None => {
                let value = frame.finish()?;
                let Some(mut parent) = parents.pop() else {
                    return Ok(value);
                };
                parent.accept(value)?;
                frame = parent;
            }
        }
    }
}

fn start(value: &InterpValue) -> Result<Started, String> {
    let frame = match value {
        InterpValue::Nominal { ty, value } => Frame::Unary {
            kind: UnaryKind::Nominal(*ty),
            source: value.clone(),
            value: None,
        },
        InterpValue::Trust { wrapper, value } => Frame::Unary {
            kind: UnaryKind::Trust(*wrapper),
            source: value.clone(),
            value: None,
        },
        InterpValue::OptionSome(value) => Frame::Unary {
            kind: UnaryKind::Some,
            source: value.clone(),
            value: None,
        },
        InterpValue::Tuple(values) => sequence(
            SequenceKind::Tuple,
            SequenceSource::Fields(values.clone()),
            values.len(),
        ),
        InterpValue::Variant { name, fields } => sequence(
            SequenceKind::Variant(name.to_string()),
            SequenceSource::Fields(fields.clone()),
            fields.len(),
        ),
        InterpValue::Array(values) => sequence(
            SequenceKind::Array,
            SequenceSource::Array(values.clone()),
            values.borrow().len(),
        ),
        InterpValue::Stack(values) => sequence(
            SequenceKind::Stack,
            SequenceSource::Array(values.clone()),
            values.borrow().len(),
        ),
        InterpValue::List(values) => sequence(
            SequenceKind::List,
            SequenceSource::List(values.clone()),
            values.len(),
        ),
        InterpValue::Slice(values) => sequence(
            SequenceKind::Slice,
            SequenceSource::Slice(values.clone()),
            values.borrow().len(),
        ),
        InterpValue::Set(values) => sequence(
            SequenceKind::Set,
            SequenceSource::Set(values.clone()),
            values.borrow().len(),
        ),
        InterpValue::OrderedSet(values) => sequence(
            SequenceKind::OrderedSet,
            SequenceSource::Set(values.clone()),
            values.borrow().len(),
        ),
        InterpValue::Deque(values) => sequence(
            SequenceKind::Deque,
            SequenceSource::Deque(values.clone()),
            values.borrow().len(),
        ),
        InterpValue::Queue(values) => sequence(
            SequenceKind::Queue,
            SequenceSource::Deque(values.clone()),
            values.borrow().len(),
        ),
        InterpValue::Map(values) => pairs(PairKind::Map, values),
        InterpValue::OrderedMap(values) => pairs(PairKind::OrderedMap, values),
        InterpValue::PriorityQueue(values) => pairs(PairKind::PriorityQueue, values),
        InterpValue::Record(values) => Frame::Record {
            source: values.clone(),
            values: Vec::with_capacity(values.borrow().len()),
        },
        _ => return ValueSnapshot::capture_leaf(value).map(Started::Value),
    };
    Ok(Started::Pending(frame))
}

fn sequence(kind: SequenceKind, source: SequenceSource, count: usize) -> Frame {
    Frame::Sequence {
        kind,
        source,
        values: Vec::with_capacity(count),
    }
}

fn pairs(kind: PairKind, source: &MapValue) -> Frame {
    Frame::Pairs {
        kind,
        source: source.clone(),
        key: None,
        values: Vec::with_capacity(source.borrow().len()),
    }
}

impl SequenceSource {
    fn next(&self, index: usize) -> Result<Option<Started>, String> {
        match self {
            Self::Fields(values) => values.get(index).map(start).transpose(),
            Self::Array(values) => values.borrow().get(index).map(start).transpose(),
            Self::Slice(values) => values.borrow().get(index).map(start).transpose(),
            Self::Set(values) => values.borrow().get(index).map(start).transpose(),
            Self::Deque(values) => values.borrow().get(index).map(start).transpose(),
            Self::List(values) => values.get(0).map(start).transpose(),
        }
    }
}

impl Frame {
    fn next(&self) -> Result<Option<Started>, String> {
        match self {
            Self::Unary {
                source,
                value: None,
                ..
            } => start(source).map(Some),
            Self::Unary { .. } => Ok(None),
            Self::Sequence { source, values, .. } => source.next(values.len()),
            Self::Pairs {
                source,
                key,
                values,
                ..
            } => source
                .borrow()
                .get(values.len())
                .map(|(k, v)| start(if key.is_some() { v } else { k }))
                .transpose(),
            Self::Record { source, values } => source
                .borrow()
                .get(values.len())
                .map(|(_, value)| start(value))
                .transpose(),
        }
    }

    fn accept(&mut self, value: ValueSnapshot) -> Result<(), String> {
        match self {
            Self::Unary { value: current, .. } => {
                if current.is_some() {
                    return Err("checkpoint unary builder received duplicate payload".into());
                }
                *current = Some(value);
            }
            Self::Sequence { source, values, .. } => {
                values.push(value);
                if let SequenceSource::List(cursor) = source {
                    if !cursor.advance() {
                        return Err("checkpoint list cursor lost its current element".into());
                    }
                }
            }
            Self::Pairs { key, values, .. } => {
                if let Some(key) = key.take() {
                    values.push((key, value));
                } else {
                    *key = Some(value);
                }
            }
            Self::Record { source, values } => {
                let fields = source.borrow();
                let (name, _) = fields
                    .get(values.len())
                    .ok_or("checkpoint record field disappeared during capture")?;
                values.push((name.clone(), value));
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<ValueSnapshot, String> {
        Ok(match self {
            Self::Unary { kind, value, .. } => {
                let value =
                    SnapshotBox::new(value.ok_or("checkpoint unary builder has no payload")?);
                match kind {
                    UnaryKind::Nominal(ty) => ValueSnapshot::Nominal { ty, value },
                    UnaryKind::Trust(wrapper) => ValueSnapshot::Trust { wrapper, value },
                    UnaryKind::Some => ValueSnapshot::OptionSome(value),
                }
            }
            Self::Sequence { kind, values, .. } => match kind {
                SequenceKind::Tuple => ValueSnapshot::Tuple(values.into()),
                SequenceKind::Array => ValueSnapshot::Array(values.into()),
                SequenceKind::List => ValueSnapshot::List(values.into()),
                SequenceKind::Slice => ValueSnapshot::Slice(values.into()),
                SequenceKind::Set => ValueSnapshot::Set(values.into()),
                SequenceKind::Deque => ValueSnapshot::Deque(values.into()),
                SequenceKind::Queue => ValueSnapshot::Queue(values.into()),
                SequenceKind::Stack => ValueSnapshot::Stack(values.into()),
                SequenceKind::OrderedSet => ValueSnapshot::OrderedSet(values.into()),
                SequenceKind::Variant(name) => ValueSnapshot::Variant {
                    name,
                    fields: values.into(),
                },
            },
            Self::Pairs {
                kind, key, values, ..
            } => {
                if key.is_some() {
                    return Err("checkpoint pair builder has an unmatched key".into());
                }
                match kind {
                    PairKind::Map => ValueSnapshot::Map(values.into()),
                    PairKind::OrderedMap => ValueSnapshot::OrderedMap(values.into()),
                    PairKind::PriorityQueue => ValueSnapshot::PriorityQueue(values.into()),
                }
            }
            Self::Record { values, .. } => ValueSnapshot::Record(values.into()),
        })
    }
}

use crate::orchestration::{
    CallTargetSnapshot, ContinuationSnapshot, LocalsSnapshot, MessageSnapshot, ValueSnapshot,
};
use serde_json::{Value, json};

mod call_target;
#[cfg(test)]
mod tests;
mod value;

pub(super) fn continuation_json(value: &ContinuationSnapshot) -> Value {
    encode(Node::Continuation(value))
}

#[derive(Clone, Copy)]
pub(super) enum Node<'a> {
    Value(&'a ValueSnapshot),
    Message(&'a MessageSnapshot),
    Frame(&'a LocalsSnapshot),
    CallTarget(&'a CallTargetSnapshot),
    Continuation(&'a ContinuationSnapshot),
    Values(&'a [ValueSnapshot]),
    NamedValues(&'a [(String, ValueSnapshot)]),
    Pairs(&'a [(ValueSnapshot, ValueSnapshot)]),
    CallTargets(&'a [CallTargetSnapshot]),
    LocalSegments(&'a [crate::orchestration::LocalPlaceSegmentSnapshot]),
}
pub(super) type Pending<'a> = Vec<(Node<'a>, &'a mut Value)>;

fn encode(node: Node<'_>) -> Value {
    let mut output = Value::Null;
    let mut pending = vec![(node, &mut output)];
    while let Some((node, slot)) = pending.pop() {
        match node {
            Node::Value(value) => value::encode(value, slot, &mut pending),
            Node::Message(message) => value::message(message, slot, &mut pending),
            Node::Frame(frame) => call_target::frame(frame, slot, &mut pending),
            Node::CallTarget(target) => call_target::encode(target, slot, &mut pending),
            Node::Continuation(value) => {
                super::machine::write_continuation(value, slot, &mut pending)
            }
            Node::Values(values) => pending.extend(
                values
                    .iter()
                    .zip(array(slot, values.len()))
                    .map(|(value, slot)| (Node::Value(value), slot)),
            ),
            Node::CallTargets(values) => pending.extend(
                values
                    .iter()
                    .zip(array(slot, values.len()))
                    .map(|(value, slot)| (Node::CallTarget(value), slot)),
            ),
            Node::NamedValues(values) => {
                for ((name, value), slot) in values.iter().zip(array(slot, values.len())) {
                    *slot = json!({"name":name, "value":null});
                    pending.push((Node::Value(value), &mut slot["value"]));
                }
            }
            Node::Pairs(values) => {
                for ((key, value), slot) in values.iter().zip(array(slot, values.len())) {
                    write_object(
                        [
                            ("key", Node::Value(key).into()),
                            ("value", Node::Value(value).into()),
                        ],
                        slot,
                        &mut pending,
                    );
                }
            }
            Node::LocalSegments(values) => {
                for (value, slot) in values.iter().zip(array(slot, values.len())) {
                    use crate::orchestration::LocalPlaceSegmentSnapshot as S;
                    match value {
                        S::Field(name) => *slot = json!({"kind":"field", "field":name}),
                        S::Index(index) => *slot = json!({"kind":"index", "index":index}),
                        S::MapKey(key) => {
                            *slot = json!({"kind":"map_key", "key":null});
                            pending.push((Node::Value(key), &mut slot["key"]));
                        }
                    }
                }
            }
        }
    }
    output
}

fn array(slot: &mut Value, count: usize) -> impl Iterator<Item = &mut Value> {
    *slot = Value::Array((0..count).map(|_| Value::Null).collect());
    match slot {
        Value::Array(values) => values.iter_mut(),
        _ => unreachable!("array slot was just initialized"),
    }
}

pub(super) enum Field<'a> {
    Json(Value),
    Node(Node<'a>),
}
impl<'a> From<Node<'a>> for Field<'a> {
    fn from(value: Node<'a>) -> Self {
        Self::Node(value)
    }
}
impl From<Value> for Field<'_> {
    fn from(value: Value) -> Self {
        Self::Json(value)
    }
}
impl<'a> From<Option<Node<'a>>> for Field<'a> {
    fn from(value: Option<Node<'a>>) -> Self {
        match value {
            Some(value) => Self::Node(value),
            None => Self::Json(Value::Null),
        }
    }
}
pub(super) fn write_object<'a, const N: usize>(
    fields: [(&'static str, Field<'a>); N],
    slot: &'a mut Value,
    pending: &mut Pending<'a>,
) {
    let mut object = serde_json::Map::new();
    let mut children = Vec::new();
    for (name, field) in fields {
        let value = match field {
            Field::Json(value) => value,
            Field::Node(node) => {
                children.push((name, node));
                Value::Null
            }
        };
        object.insert(name.to_owned(), value);
    }
    *slot = Value::Object(object);
    if let Value::Object(fields) = slot {
        for (name, slot) in fields {
            if let Some((_, node)) = children.iter().find(|(key, _)| *key == name) {
                pending.push((*node, slot));
            }
        }
    }
}

use super::value::{encode_host, encode_model};
use crate::orchestration::{
    CallTargetSnapshot, ContinuationSnapshot, LocalsSnapshot, MessageSnapshot, ValueSnapshot,
};
use serde_json::{Value, json};

mod budget;
mod call_target;
#[cfg(test)]
mod tests;
mod value;
pub(super) use budget::EncodingBudget;

#[cfg(test)]
pub(super) fn continuation_json(value: &ContinuationSnapshot) -> Value {
    encode(Node::Continuation(value))
}

pub(super) fn continuation_json_with_budget(
    value: &ContinuationSnapshot,
    budget: &mut EncodingBudget,
) -> Result<Value, super::InterpreterCodecError> {
    encode_with_budget(Node::Continuation(value), budget)
}

#[derive(Clone, Copy)]
pub(super) enum Node<'a> {
    Value(&'a ValueSnapshot),
    Json(&'a crate::value::HostJsonSupportValue),
    Host(&'a crate::value::HostSupportValue),
    Model(encode_model::Part<'a>),
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

#[cfg(test)]
fn encode(node: Node<'_>) -> Value {
    encode_with_budget(node, &mut EncodingBudget::default())
        .expect("test snapshot fits encoding budget")
}

fn encode_with_budget(
    node: Node<'_>,
    budget: &mut EncodingBudget,
) -> Result<Value, super::InterpreterCodecError> {
    let mut output = super::CheckpointDocument::from_value(Value::Null);
    let mut pending = vec![(node, output.value_mut())];
    while let Some((node, slot)) = pending.pop() {
        budget.admit(node, pending.len())?;
        match node {
            Node::Value(value) => value::encode(value, slot, &mut pending),
            Node::Json(value) => super::json::write(value, slot, |value, slot| {
                pending.push((Node::Json(value), slot));
            }),
            Node::Host(value) => {
                encode_host::write(encode_host::Node::Host(value), slot, |node, slot| {
                    let node = match node {
                        encode_host::Node::Host(value) => Node::Host(value),
                        encode_host::Node::Json(value) => Node::Json(value),
                    };
                    pending.push((node, slot));
                })
            }
            Node::Model(part) => model(part, slot, &mut pending),
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
    Ok(output.into_value())
}

fn model<'a>(part: encode_model::Part<'a>, slot: &'a mut Value, pending: &mut Pending<'a>) {
    encode_model::write(part, slot, |child, slot| {
        let node = match child {
            encode_model::Child::Model(part) => Node::Model(part),
            encode_model::Child::Host(value) => Node::Host(value),
        };
        pending.push((node, slot));
    });
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

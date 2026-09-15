use crate::api::codec::{CheckpointDocument, json as json_codec};
use serde_json::{Value, json};

mod source;
use source::{HostSource, HostView};

#[cfg(test)]
mod tests;

enum Node<'a, T: HostSource> {
    Host(&'a T),
    Json(&'a T::Json),
}

// Write directly to final output slots; the worklist only borrows input nodes.
// Host containers and embedded JSON share the same traversal, so neither creates
// an intermediate serialized subtree at their boundary.
pub(super) fn encode<T: HostSource>(value: &T) -> Value {
    let mut output = CheckpointDocument::from_value(Value::Null);
    let mut pending = vec![(Node::Host(value), output.value_mut())];
    while let Some((node, slot)) = pending.pop() {
        let value = match node {
            Node::Host(value) => value,
            Node::Json(value) => {
                json_codec::write(value, slot, |value, slot| {
                    pending.push((Node::Json(value), slot));
                });
                continue;
            }
        };
        match value.view() {
            HostView::Leaf(value) => *slot = value,
            HostView::List(values) => {
                *slot = json!({"kind":"list","values":null});
                pending.extend(
                    values
                        .iter()
                        .zip(array(&mut slot["values"], values.len()))
                        .map(|(value, slot)| (Node::Host(value), slot)),
                );
            }
            HostView::Variant { name, fields } => {
                *slot = json!({"kind":"variant","name":name,"fields":null});
                pending.extend(
                    fields
                        .iter()
                        .zip(array(&mut slot["fields"], fields.len()))
                        .map(|(value, slot)| (Node::Host(value), slot)),
                );
            }
            HostView::Record(fields) => {
                *slot = json!({"kind":"record","fields":null});
                for ((name, value), slot) in
                    fields.iter().zip(array(&mut slot["fields"], fields.len()))
                {
                    *slot = json!({"name":name,"value":null});
                    pending.push((Node::Host(value), &mut slot["value"]));
                }
            }
            HostView::Map(entries) => {
                *slot = json!({"kind":"map","entries":null});
                for ((key, value), slot) in entries
                    .iter()
                    .zip(array(&mut slot["entries"], entries.len()))
                {
                    *slot = json!({"key":null,"value":null});
                    let Value::Object(fields) = slot else {
                        unreachable!("entry object initialized above")
                    };
                    for (name, slot) in fields {
                        pending.push((Node::Host(if name == "key" { key } else { value }), slot));
                    }
                }
            }
            HostView::Json(value) => {
                *slot = json!({"kind":"json","value":null});
                pending.push((Node::Json(value), &mut slot["value"]));
            }
        }
    }
    output.into_value()
}

fn array(slot: &mut Value, count: usize) -> impl Iterator<Item = &mut Value> {
    *slot = Value::Array((0..count).map(|_| Value::Null).collect());
    match slot {
        Value::Array(values) => values.iter_mut(),
        _ => unreachable!("array initialized above"),
    }
}

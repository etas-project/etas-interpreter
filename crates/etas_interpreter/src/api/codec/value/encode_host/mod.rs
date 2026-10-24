use crate::api::codec::{CheckpointDocument, json as json_codec};
use serde_json::{Value, json};

mod source;
use source::{HostSource, HostView};

#[cfg(test)]
mod tests;

pub(in crate::api::codec) enum Node<'a, T: HostSource> {
    Host(&'a T),
    Json(&'a T::Json),
}

// Write directly to final output slots; the worklist only borrows input nodes.
// Host containers and embedded JSON share the same traversal, so neither creates
// an intermediate serialized subtree at their boundary.
pub(in crate::api::codec) fn encode<T: HostSource>(value: &T) -> Value {
    let mut output = CheckpointDocument::from_value(Value::Null);
    let mut pending = vec![(Node::Host(value), output.value_mut())];
    while let Some((node, slot)) = pending.pop() {
        write(node, slot, |node, slot| pending.push((node, slot)));
    }
    output.into_value()
}

pub(in crate::api::codec) fn write<'a, T: HostSource>(
    node: Node<'a, T>,
    slot: &'a mut Value,
    mut child: impl FnMut(Node<'a, T>, &'a mut Value),
) {
    let value = match node {
        Node::Host(value) => value,
        Node::Json(value) => {
            json_codec::write(value, slot, |value, slot| {
                child(Node::Json(value), slot);
            });
            return;
        }
    };
    match value.view() {
        HostView::Leaf(value) => *slot = value,
        HostView::List(values) => {
            *slot = json!({"kind":"list","values":null});
            for (value, slot) in values.iter().zip(array(&mut slot["values"], values.len())) {
                child(Node::Host(value), slot);
            }
        }
        HostView::Variant { name, fields } => {
            *slot = json!({"kind":"variant","name":name,"fields":null});
            for (value, slot) in fields.iter().zip(array(&mut slot["fields"], fields.len())) {
                child(Node::Host(value), slot);
            }
        }
        HostView::Record(fields) => {
            *slot = json!({"kind":"record","fields":null});
            for ((name, value), slot) in fields.iter().zip(array(&mut slot["fields"], fields.len()))
            {
                *slot = json!({"name":name,"value":null});
                child(Node::Host(value), &mut slot["value"]);
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
                    child(Node::Host(if name == "key" { key } else { value }), slot);
                }
            }
        }
        HostView::Json(value) => {
            *slot = json!({"kind":"json","value":null});
            child(Node::Json(value), &mut slot["value"]);
        }
    }
}

fn array(slot: &mut Value, count: usize) -> impl Iterator<Item = &mut Value> {
    *slot = Value::Array((0..count).map(|_| Value::Null).collect());
    match slot {
        Value::Array(values) => values.iter_mut(),
        _ => unreachable!("array initialized above"),
    }
}

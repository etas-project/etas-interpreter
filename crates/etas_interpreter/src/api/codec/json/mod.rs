use super::CheckpointDocument;
use serde_json::{Value, json};

mod view;
use view::{JsonSource, JsonView};

#[cfg(test)]
pub(super) mod tests;

// Ordinary runtime reporting keeps its existing infallible API. Checkpoint
// encoding calls `write` from its budgeted worklist instead of using this entry.
pub(super) fn wrapped<T: JsonSource>(value: &T) -> Value {
    let mut output = CheckpointDocument::from_value(json!({"kind":"json","value":null}));
    let mut pending = vec![(value, &mut output.value_mut()["value"])];
    while let Some((value, slot)) = pending.pop() {
        write(value, slot, |value, slot| pending.push((value, slot)));
    }
    output.into_value()
}

pub(super) fn write<'a, T: JsonSource>(
    value: &'a T,
    slot: &'a mut Value,
    mut child: impl FnMut(&'a T, &'a mut Value),
) {
    match value.view() {
        JsonView::Null => *slot = json!({"kind":"null"}),
        JsonView::Bool(value) => *slot = json!({"kind":"bool","value":value}),
        JsonView::NumberBits(value) => *slot = json!({"kind":"number_bits","value":value}),
        JsonView::String(value) => *slot = json!({"kind":"string","value":value}),
        JsonView::Array(values) => {
            *slot = json!({"kind":"array","values":null});
            for (value, slot) in values.iter().zip(array(&mut slot["values"], values.len())) {
                child(value, slot);
            }
        }
        JsonView::Object(entries) => {
            *slot = json!({"kind":"object","entries":null});
            for ((key, value), entry) in entries
                .iter()
                .zip(array(&mut slot["entries"], entries.len()))
            {
                *entry = json!({"key":key,"value":null});
                child(value, &mut entry["value"]);
            }
        }
    }
}

fn array(slot: &mut Value, count: usize) -> impl Iterator<Item = &mut Value> {
    *slot = Value::Array((0..count).map(|_| Value::Null).collect());
    match slot {
        Value::Array(values) => values.iter_mut(),
        _ => unreachable!("array slot was initialized above"),
    }
}

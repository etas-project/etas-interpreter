use super::{Node, Pending, array};
use crate::value::HostJsonSupportValue as Json;
use serde_json::{Value, json};

pub(super) fn encode<'a>(value: &'a Json, slot: &'a mut Value, pending: &mut Pending<'a>) {
    match value {
        Json::Null => *slot = json!({"kind":"null"}),
        Json::Bool(value) => *slot = json!({"kind":"bool","value":value}),
        Json::NumberBits(value) => *slot = json!({"kind":"number_bits","value":value}),
        Json::String(value) => *slot = json!({"kind":"string","value":value}),
        Json::Array(values) => {
            *slot = json!({"kind":"array","values":null});
            pending.extend(
                values
                    .iter()
                    .zip(array(&mut slot["values"], values.len()))
                    .map(|(value, slot)| (Node::Json(value), slot)),
            );
        }
        Json::Object(entries) => {
            *slot = json!({"kind":"object","entries":null});
            for ((key, value), entry) in entries
                .iter()
                .zip(array(&mut slot["entries"], entries.len()))
            {
                *entry = json!({"key":key,"value":null});
                pending.push((Node::Json(value), &mut entry["value"]));
            }
        }
    }
}

use super::encode_host;
use crate::api::codec::CheckpointDocument;
use crate::value::{
    HostSupportValue, ModelContentValue, ModelMessageValue, ModelResponseValue, ModelToolCallValue,
    codec,
};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub(in crate::api::codec) enum Part<'a> {
    Response(&'a ModelResponseValue),
    Message(&'a ModelMessageValue),
    Content(&'a ModelContentValue),
    ToolCall(&'a ModelToolCallValue),
}

pub(in crate::api::codec) enum Child<'a> {
    Model(Part<'a>),
    Host(&'a HostSupportValue),
}

enum Work<'a> {
    Model(Part<'a>),
    Host(encode_host::Node<'a, HostSupportValue>),
}

pub(in crate::api::codec) fn model_response_json(response: &ModelResponseValue) -> Value {
    let mut output = CheckpointDocument::from_value(Value::Null);
    let mut pending = vec![(Work::Model(Part::Response(response)), output.value_mut())];
    while let Some((node, slot)) = pending.pop() {
        match node {
            Work::Model(part) => write(part, slot, |child, slot| {
                let node = match child {
                    Child::Model(part) => Work::Model(part),
                    Child::Host(value) => Work::Host(encode_host::Node::Host(value)),
                };
                pending.push((node, slot));
            }),
            Work::Host(node) => encode_host::write(node, slot, |node, slot| {
                pending.push((Work::Host(node), slot))
            }),
        }
    }
    output.into_value()
}

// Checkpoint encoding calls the same slot writer from its own budgeted worklist.
pub(in crate::api::codec) fn write<'a>(
    part: Part<'a>,
    slot: &'a mut Value,
    mut child: impl FnMut(Child<'a>, &'a mut Value),
) {
    match part {
        Part::Response(response) => {
            *slot = json!({"kind":"model_response","id":response.id,"message":null,"tool_calls":null,
                "usage":response.usage.as_ref().map(|usage| json!({"input_tokens":usage.input_tokens,"output_tokens":usage.output_tokens}))});
            let Value::Object(fields) = slot else {
                unreachable!("model response object initialized above")
            };
            for (name, slot) in fields {
                match name.as_str() {
                    "message" => child(Child::Model(Part::Message(&response.message)), slot),
                    "tool_calls" => {
                        for (call, slot) in response
                            .tool_calls
                            .iter()
                            .zip(array(slot, response.tool_calls.len()))
                        {
                            child(Child::Model(Part::ToolCall(call)), slot);
                        }
                    }
                    _ => {}
                }
            }
        }
        Part::Message(message) => {
            *slot = json!({"role":codec::model_role_json(message.role),"content":null});
            for (content, slot) in message
                .content
                .iter()
                .zip(array(&mut slot["content"], message.content.len()))
            {
                child(Child::Model(Part::Content(content)), slot);
            }
        }
        Part::Content(ModelContentValue::Text(text)) => *slot = json!({"kind":"text","text":text}),
        Part::Content(ModelContentValue::Value(value)) => {
            *slot = json!({"kind":"value","value":null});
            child(Child::Host(value), &mut slot["value"]);
        }
        Part::ToolCall(call) => {
            *slot = json!({"id":call.id,"tool":call.tool,"args":null});
            child(Child::Host(&call.args), &mut slot["args"]);
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

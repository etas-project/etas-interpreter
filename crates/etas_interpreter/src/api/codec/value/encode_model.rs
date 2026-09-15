use super::host_support_value_json;
use crate::api::codec::CheckpointDocument;
use crate::value::{
    ModelContentValue, ModelMessageValue, ModelResponseValue, ModelToolCallValue, codec,
};
use serde_json::{Value, json};

pub(in crate::api::codec) fn model_response_json(response: &ModelResponseValue) -> Value {
    let mut output = CheckpointDocument::from_value(json!({
        "kind":"model_response",
        "id":response.id,
        "message":null,
        "tool_calls":null,
        "usage":response.usage.as_ref().map(|usage| json!({
            "input_tokens":usage.input_tokens,
            "output_tokens":usage.output_tokens,
        })),
    }));
    output.value_mut()["message"] = model_message_json(&response.message);
    output.value_mut()["tool_calls"] = Value::Array(
        response
            .tool_calls
            .iter()
            .map(model_tool_call_json)
            .collect(),
    );
    output.into_value()
}

fn model_message_json(message: &ModelMessageValue) -> Value {
    let mut output = json!({"role":codec::model_role_json(message.role),"content":null});
    output["content"] = Value::Array(message.content.iter().map(model_content_json).collect());
    output
}

fn model_content_json(content: &ModelContentValue) -> Value {
    match content {
        ModelContentValue::Text(text) => json!({"kind":"text","text":text}),
        ModelContentValue::Value(value) => {
            let mut output = json!({"kind":"value","value":null});
            output["value"] = host_support_value_json(value);
            output
        }
    }
}

fn model_tool_call_json(call: &ModelToolCallValue) -> Value {
    let mut output = json!({"id":call.id,"tool":call.tool,"args":null});
    output["args"] = host_support_value_json(&call.args);
    output
}

use serde_json::{Value, json};

use crate::value::StringValue;

pub(in crate::api::codec) trait JsonText {
    fn json_text(&self) -> Value;
}

impl JsonText for str {
    fn json_text(&self) -> Value {
        Value::String(self.to_owned())
    }
}

impl JsonText for StringValue {
    fn json_text(&self) -> Value {
        // A DOM needs owned text, not a cached input or a growing fmt buffer.
        Value::String(self.clone().into_string())
    }
}

pub(in crate::api::codec) fn tagged_text<T: JsonText + ?Sized>(kind: &str, text: &T) -> Value {
    let mut value = json!({"kind":kind,"value":null});
    value["value"] = text.json_text();
    value
}

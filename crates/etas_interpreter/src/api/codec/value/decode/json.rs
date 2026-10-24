use super::super::*;
use crate::value::HostJsonSupportValue as Json;

pub(in crate::api::codec) fn decode(root: &Value) -> Result<Json, InterpreterCodecError> {
    let mut frames = Vec::new();
    let mut next = root;
    loop {
        let mut result = match required_str(next, "kind")? {
            "null" => Json::Null,
            "bool" => Json::Bool(required_bool(next, "value")?),
            "number_bits" => Json::NumberBits(required_u64(next, "value")?),
            "string" => Json::String(required_str(next, "value")?.into()),
            kind @ ("array" | "object") => {
                let input =
                    required_array(next, if kind == "array" { "values" } else { "entries" })?;
                let frame = if kind == "array" {
                    Frame::Array {
                        input,
                        output: Vec::with_capacity(input.len()),
                    }
                } else {
                    Frame::Object {
                        input,
                        output: Vec::with_capacity(input.len()),
                    }
                };
                if let Some(child) = frame.next_child()? {
                    next = child;
                    frames.push(frame);
                    continue;
                }
                frame.finish()
            }
            other => {
                return Err(InterpreterCodecError::new(format!(
                    "unsupported host json support value `{other}`"
                )));
            }
        };
        loop {
            let Some(mut frame) = frames.pop() else {
                return Ok(result);
            };
            frame.accept(result)?;
            if let Some(child) = frame.next_child()? {
                next = child;
                frames.push(frame);
                break;
            }
            result = frame.finish();
        }
    }
}

enum Frame<'a> {
    Array {
        input: &'a [Value],
        output: Vec<Json>,
    },
    Object {
        input: &'a [Value],
        output: Vec<(String, Json)>,
    },
}

impl<'a> Frame<'a> {
    fn next_child(&self) -> Result<Option<&'a Value>, InterpreterCodecError> {
        match self {
            Self::Array { input, output } => Ok(input.get(output.len())),
            Self::Object { input, output } => input
                .get(output.len())
                .map(|entry| {
                    // Validate labels before values, matching the source wire order.
                    required_str(entry, "key")?;
                    required_obj(entry, "value")
                })
                .transpose(),
        }
    }

    fn accept(&mut self, value: Json) -> Result<(), InterpreterCodecError> {
        match self {
            Self::Array { output, .. } => output.push(value),
            Self::Object { input, output } => {
                let key = required_str(&input[output.len()], "key")?;
                output.push((key.to_owned(), value));
            }
        }
        Ok(())
    }

    fn finish(self) -> Json {
        match self {
            Self::Array { output, .. } => Json::Array(output.into()),
            Self::Object { output, .. } => Json::Object(output.into()),
        }
    }
}

use super::super::*;
use crate::value::HostSupportValue as H;
#[cfg(test)]
mod tests;

pub(in crate::api::codec) fn decode(root: &Value) -> Result<H, InterpreterCodecError> {
    let mut frames = Vec::new();
    let mut next = root;
    loop {
        let mut value = match required_str(next, "kind")? {
            "unit" => H::Unit,
            "bool" => H::Bool(required_bool(next, "value")?),
            "int" => H::Int(required_str(next, "value")?.into()),
            "uint" => H::UInt(required_str(next, "value")?.into()),
            "float_bits" => H::FloatBits(required_u64(next, "value")?),
            "string" => H::String(required_str(next, "value")?.into()),
            "bytes" => H::Bytes(byte_array(next, "value")?.into()),
            "json" => H::Json(super::json::decode(required_obj(next, "value")?)?),
            kind @ ("list" | "map" | "record" | "variant") => {
                let frame = Frame::new(kind, next)?;
                if let Some(child) = frame.next()? {
                    frames.push(frame);
                    next = child;
                    continue;
                }
                frame.finish()
            }
            other => {
                return Err(InterpreterCodecError::new(format!(
                    "unsupported host support value `{other}`"
                )));
            }
        };
        loop {
            let Some(mut frame) = frames.pop() else {
                return Ok(value);
            };
            frame.accept(value)?;
            if let Some(child) = frame.next()? {
                frames.push(frame);
                next = child;
                break;
            }
            value = frame.finish();
        }
    }
}

enum Frame<'a> {
    Sequence {
        input: &'a [Value],
        values: Vec<H>,
        name: Option<&'a str>,
    },
    Map {
        input: &'a [Value],
        entries: Vec<(H, H)>,
        key: Option<H>,
    },
    Record {
        input: &'a [Value],
        fields: Vec<(String, H)>,
    },
}

impl<'a> Frame<'a> {
    fn new(kind: &str, value: &'a Value) -> Result<Self, InterpreterCodecError> {
        Ok(match kind {
            "list" | "variant" => {
                let name = (kind == "variant")
                    .then(|| required_str(value, "name"))
                    .transpose()?;
                let input =
                    required_array(value, if name.is_some() { "fields" } else { "values" })?;
                Self::Sequence {
                    input,
                    values: Vec::with_capacity(input.len()),
                    name,
                }
            }
            "map" => {
                let input = required_array(value, "entries")?;
                Self::Map {
                    input,
                    entries: Vec::with_capacity(input.len()),
                    key: None,
                }
            }
            "record" => {
                let input = required_array(value, "fields")?;
                Self::Record {
                    input,
                    fields: Vec::with_capacity(input.len()),
                }
            }
            _ => {
                return Err(InterpreterCodecError::new(
                    "invalid Host payload builder kind",
                ));
            }
        })
    }

    fn next(&self) -> Result<Option<&'a Value>, InterpreterCodecError> {
        match self {
            Self::Sequence { input, values, .. } => Ok(input.get(values.len())),
            Self::Map {
                input,
                entries,
                key,
            } => input
                .get(entries.len())
                .map(|entry| required_obj(entry, if key.is_some() { "value" } else { "key" }))
                .transpose(),
            Self::Record { input, fields } => input
                .get(fields.len())
                .map(|field| {
                    required_str(field, "name")?;
                    required_obj(field, "value")
                })
                .transpose(),
        }
    }

    fn accept(&mut self, value: H) -> Result<(), InterpreterCodecError> {
        match self {
            Self::Sequence { values, .. } => values.push(value),
            Self::Map { entries, key, .. } => {
                if let Some(key) = key.take() {
                    entries.push((key, value));
                } else {
                    *key = Some(value);
                }
            }
            Self::Record { input, fields } => {
                // `next` validates the label before descending into this child.
                let field = input.get(fields.len()).ok_or_else(|| {
                    InterpreterCodecError::new("missing Host field builder input")
                })?;
                let name = required_str(field, "name")?;
                fields.push((name.to_owned(), value));
            }
        }
        Ok(())
    }

    fn finish(self) -> H {
        match self {
            Self::Sequence {
                values,
                name: Some(name),
                ..
            } => H::Variant {
                name: name.into(),
                fields: values.into(),
            },
            Self::Sequence {
                values, name: None, ..
            } => H::List(values.into()),
            Self::Map { entries, .. } => H::Map(entries.into()),
            Self::Record { fields, .. } => H::Record(fields.into()),
        }
    }
}

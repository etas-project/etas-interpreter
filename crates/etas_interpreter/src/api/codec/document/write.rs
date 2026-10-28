use std::io::{self, Write};

use serde_json::Value;

enum Pending<'a> {
    Value(&'a Value),
    Array(std::slice::Iter<'a, Value>, bool),
    Object(serde_json::map::Iter<'a>, bool),
}

pub(super) fn json(mut output: impl Write, root: &Value) -> io::Result<()> {
    let mut pending = vec![Pending::Value(root)];
    while let Some(next) = pending.pop() {
        match next {
            Pending::Value(Value::Array(values)) => {
                output.write_all(b"[")?;
                pending.push(Pending::Array(values.iter(), true));
            }
            Pending::Value(Value::Object(fields)) => {
                output.write_all(b"{")?;
                pending.push(Pending::Object(fields.iter(), true));
            }
            Pending::Value(scalar) => {
                serde_json::to_writer(&mut output, scalar).map_err(json_error)?;
            }
            Pending::Array(mut values, first) => match values.next() {
                Some(value) => {
                    if !first {
                        output.write_all(b",")?;
                    }
                    pending.push(Pending::Array(values, false));
                    pending.push(Pending::Value(value));
                }
                None => output.write_all(b"]")?,
            },
            Pending::Object(mut fields, first) => match fields.next() {
                Some((key, value)) => {
                    if !first {
                        output.write_all(b",")?;
                    }
                    serde_json::to_writer(&mut output, key).map_err(json_error)?;
                    output.write_all(b":")?;
                    pending.push(Pending::Object(fields, false));
                    pending.push(Pending::Value(value));
                }
                None => output.write_all(b"}")?,
            },
        }
    }
    Ok(())
}

fn json_error(error: serde_json::Error) -> io::Error {
    match error.io_error_kind() {
        Some(kind) => io::Error::new(kind, error),
        None => io::Error::new(io::ErrorKind::InvalidData, error),
    }
}

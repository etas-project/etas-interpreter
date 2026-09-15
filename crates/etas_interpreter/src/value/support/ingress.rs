use super::{HostJsonSupportValue as Json, HostSupportValue as H};
use etas_host::{HostJsonValue as J, HostValue as V};

enum Frame {
    Sequence {
        rest: std::vec::IntoIter<V>,
        values: Vec<H>,
        name: Option<String>,
    },
    Record {
        rest: std::vec::IntoIter<(String, V)>,
        values: Vec<(String, H)>,
        name: String,
    },
    MapKey {
        rest: std::vec::IntoIter<(V, V)>,
        values: Vec<(H, H)>,
        value: V,
    },
    MapValue {
        rest: std::vec::IntoIter<(V, V)>,
        values: Vec<(H, H)>,
        key: H,
    },
}

impl From<V> for H {
    fn from(mut next: V) -> Self {
        let mut frames = Vec::new();
        loop {
            let mut output = match next {
                V::Unit => H::Unit,
                V::Bool(v) => H::Bool(v),
                V::Int(v) => H::Int(v.to_string().into()),
                V::UInt(v) => H::UInt(v.to_string().into()),
                V::Float(v) => H::FloatBits(v.to_bits()),
                V::String(v) => H::String(v.into()),
                V::Bytes(v) => H::Bytes(v.into()),
                V::Json(v) => H::Json(v.into()),
                value @ (V::List(_) | V::Variant { .. }) => {
                    let (name, values) = match value {
                        V::List(values) => (None, values),
                        V::Variant { name, fields } => (Some(name), fields),
                        _ => unreachable!("sequence variants matched above"),
                    };
                    let output = Vec::with_capacity(values.len());
                    let mut rest = values.into_iter();
                    if let Some(child) = rest.next() {
                        frames.push(Frame::Sequence {
                            rest,
                            values: output,
                            name,
                        });
                        next = child;
                        continue;
                    }
                    sequence(name, output)
                }
                V::Record(values) => {
                    let output = Vec::with_capacity(values.len());
                    let mut rest = values.into_iter();
                    if let Some((name, child)) = rest.next() {
                        frames.push(Frame::Record {
                            rest,
                            values: output,
                            name,
                        });
                        next = child;
                        continue;
                    }
                    H::Record(output.into())
                }
                V::Map(values) => {
                    let output = Vec::with_capacity(values.len());
                    let mut rest = values.into_iter();
                    if let Some((key, value)) = rest.next() {
                        frames.push(Frame::MapKey {
                            rest,
                            values: output,
                            value,
                        });
                        next = key;
                        continue;
                    }
                    H::Map(output.into())
                }
            };
            loop {
                let Some(frame) = frames.pop() else {
                    return output;
                };
                match frame {
                    Frame::Sequence {
                        mut rest,
                        mut values,
                        name,
                    } => {
                        values.push(output);
                        if let Some(child) = rest.next() {
                            frames.push(Frame::Sequence { rest, values, name });
                            next = child;
                            break;
                        }
                        output = sequence(name, values);
                    }
                    Frame::Record {
                        mut rest,
                        mut values,
                        name,
                    } => {
                        values.push((name, output));
                        if let Some((name, child)) = rest.next() {
                            frames.push(Frame::Record { rest, values, name });
                            next = child;
                            break;
                        }
                        output = H::Record(values.into());
                    }
                    Frame::MapKey {
                        rest,
                        values,
                        value,
                    } => {
                        frames.push(Frame::MapValue {
                            rest,
                            values,
                            key: output,
                        });
                        next = value;
                        break;
                    }
                    Frame::MapValue {
                        mut rest,
                        mut values,
                        key,
                    } => {
                        values.push((key, output));
                        if let Some((key, value)) = rest.next() {
                            frames.push(Frame::MapKey {
                                rest,
                                values,
                                value,
                            });
                            next = key;
                            break;
                        }
                        output = H::Map(values.into());
                    }
                }
            }
        }
    }
}

fn sequence(name: Option<String>, values: Vec<H>) -> H {
    match name {
        Some(name) => H::Variant {
            name: name.into(),
            fields: values.into(),
        },
        None => H::List(values.into()),
    }
}

enum JsonFrame {
    Array {
        rest: std::vec::IntoIter<J>,
        values: Vec<Json>,
    },
    Object {
        rest: std::vec::IntoIter<(String, J)>,
        values: Vec<(String, Json)>,
        name: String,
    },
}

impl From<J> for Json {
    fn from(mut next: J) -> Self {
        let mut frames = Vec::new();
        loop {
            let mut output = match next {
                J::Null => Json::Null,
                J::Bool(v) => Json::Bool(v),
                J::Number(v) => Json::NumberBits(v.to_bits()),
                J::String(v) => Json::String(v.into()),
                J::Array(values) => {
                    let output = Vec::with_capacity(values.len());
                    let mut rest = values.into_iter();
                    if let Some(child) = rest.next() {
                        frames.push(JsonFrame::Array {
                            rest,
                            values: output,
                        });
                        next = child;
                        continue;
                    }
                    Json::Array(output.into())
                }
                J::Object(values) => {
                    let output = Vec::with_capacity(values.len());
                    let mut rest = values.into_iter();
                    if let Some((name, child)) = rest.next() {
                        frames.push(JsonFrame::Object {
                            rest,
                            values: output,
                            name,
                        });
                        next = child;
                        continue;
                    }
                    Json::Object(output.into())
                }
            };
            loop {
                let Some(frame) = frames.pop() else {
                    return output;
                };
                match frame {
                    JsonFrame::Array {
                        mut rest,
                        mut values,
                    } => {
                        values.push(output);
                        if let Some(child) = rest.next() {
                            frames.push(JsonFrame::Array { rest, values });
                            next = child;
                            break;
                        }
                        output = Json::Array(values.into());
                    }
                    JsonFrame::Object {
                        mut rest,
                        mut values,
                        name,
                    } => {
                        values.push((name, output));
                        if let Some((name, child)) = rest.next() {
                            frames.push(JsonFrame::Object { rest, values, name });
                            next = child;
                            break;
                        }
                        output = Json::Object(values.into());
                    }
                }
            }
        }
    }
}

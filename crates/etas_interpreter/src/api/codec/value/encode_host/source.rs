use crate::api::codec::json::view::JsonSource;
use crate::value::{HostJsonSupportValue, HostSupportValue};
use etas_host::{HostJsonValue, HostValue};
use serde_json::{Value, json};

pub(in crate::api::codec) enum HostView<'a, T: HostSource> {
    Leaf(Value),
    List(&'a [T]),
    Map(&'a [(T, T)]),
    Record(&'a [(String, T)]),
    Variant { name: &'a str, fields: &'a [T] },
    Json(&'a T::Json),
}

pub(in crate::api::codec) trait HostSource: Sized {
    type Json: JsonSource;
    fn view(&self) -> HostView<'_, Self>;
}

impl HostSource for HostValue {
    type Json = HostJsonValue;
    fn view(&self) -> HostView<'_, Self> {
        HostView::Leaf(match self {
            Self::Unit => json!({"kind":"unit"}),
            Self::Bool(value) => json!({"kind":"bool","value":value}),
            Self::Int(value) => json!({"kind":"int","value":value.to_string()}),
            Self::UInt(value) => json!({"kind":"uint","value":value.to_string()}),
            Self::Float(value) => json!({"kind":"float_bits","value":value.to_bits()}),
            Self::String(value) => json!({"kind":"string","value":value}),
            Self::Bytes(value) => json!({"kind":"bytes","value":value}),
            Self::List(values) => return HostView::List(values),
            Self::Map(entries) => return HostView::Map(entries),
            Self::Record(fields) => return HostView::Record(fields),
            Self::Variant { name, fields } => return HostView::Variant { name, fields },
            Self::Json(value) => return HostView::Json(value),
        })
    }
}

impl HostSource for HostSupportValue {
    type Json = HostJsonSupportValue;
    fn view(&self) -> HostView<'_, Self> {
        HostView::Leaf(match self {
            Self::Unit => json!({"kind":"unit"}),
            Self::Bool(value) => json!({"kind":"bool","value":value}),
            Self::Int(value) => json!({"kind":"int","value":value}),
            Self::UInt(value) => json!({"kind":"uint","value":value}),
            Self::FloatBits(value) => json!({"kind":"float_bits","value":value}),
            Self::String(value) => json!({"kind":"string","value":value}),
            Self::Bytes(value) => json!({"kind":"bytes","value":value}),
            Self::List(values) => return HostView::List(values),
            Self::Map(entries) => return HostView::Map(entries),
            Self::Record(fields) => return HostView::Record(fields),
            Self::Variant { name, fields } => return HostView::Variant { name, fields },
            Self::Json(value) => return HostView::Json(value),
        })
    }
}

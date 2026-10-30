use crate::api::codec::json::view::JsonSource;
use crate::api::codec::text::{JsonText, tagged_text};
use crate::value::{HostJsonSupportValue, HostSupportValue};
use etas_host::{HostJsonValue, HostValue};
use serde_json::{Value, json};

pub(in crate::api::codec) enum HostView<'a, T: HostSource> {
    Leaf(Value),
    List(&'a [T]),
    Map(&'a [(T, T)]),
    Record(&'a [(String, T)]),
    Variant { name: &'a T::Text, fields: &'a [T] },
    Json(&'a T::Json),
}

pub(in crate::api::codec) trait HostSource: Sized {
    type Json: JsonSource;
    type Text: JsonText + ?Sized;
    fn view(&self) -> HostView<'_, Self>;
}

impl HostSource for HostValue {
    type Json = HostJsonValue;
    type Text = str;
    fn view(&self) -> HostView<'_, Self> {
        HostView::Leaf(match self {
            Self::Unit => json!({"kind":"unit"}),
            Self::Bool(value) => json!({"kind":"bool","value":value}),
            Self::Int(value) => json!({"kind":"int","value":value.to_string()}),
            Self::UInt(value) => json!({"kind":"uint","value":value.to_string()}),
            Self::Float(value) => json!({"kind":"float_bits","value":value.to_bits()}),
            Self::String(value) => tagged_text("string", value.as_str()),
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
    type Text = crate::value::StringValue;
    fn view(&self) -> HostView<'_, Self> {
        HostView::Leaf(match self {
            Self::Unit => json!({"kind":"unit"}),
            Self::Bool(value) => json!({"kind":"bool","value":value}),
            Self::Int(value) => tagged_text("int", value),
            Self::UInt(value) => tagged_text("uint", value),
            Self::FloatBits(value) => json!({"kind":"float_bits","value":value}),
            Self::String(value) => tagged_text("string", value),
            Self::Bytes(value) => json!({"kind":"bytes","value":value}),
            Self::List(values) => return HostView::List(values),
            Self::Map(entries) => return HostView::Map(entries),
            Self::Record(fields) => return HostView::Record(fields),
            Self::Variant { name, fields } => return HostView::Variant { name, fields },
            Self::Json(value) => return HostView::Json(value),
        })
    }
}

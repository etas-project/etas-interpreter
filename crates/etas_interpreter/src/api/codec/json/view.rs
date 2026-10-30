use crate::api::codec::text::JsonText;
use crate::value::HostJsonSupportValue;
use etas_host::HostJsonValue;

pub(in crate::api::codec) enum JsonView<'a, T: JsonSource> {
    Null,
    Bool(bool),
    NumberBits(u64),
    String(&'a T::Text),
    Array(&'a [T]),
    Object(&'a [(String, T)]),
}

pub(in crate::api::codec) trait JsonSource: Sized {
    type Text: JsonText + ?Sized;
    fn view(&self) -> JsonView<'_, Self>;
}

impl JsonSource for HostJsonSupportValue {
    type Text = crate::value::StringValue;
    fn view(&self) -> JsonView<'_, Self> {
        match self {
            Self::Null => JsonView::Null,
            Self::Bool(value) => JsonView::Bool(*value),
            Self::NumberBits(value) => JsonView::NumberBits(*value),
            Self::String(value) => JsonView::String(value),
            Self::Array(values) => JsonView::Array(values),
            Self::Object(entries) => JsonView::Object(entries),
        }
    }
}

impl JsonSource for HostJsonValue {
    type Text = str;
    fn view(&self) -> JsonView<'_, Self> {
        match self {
            Self::Null => JsonView::Null,
            Self::Bool(value) => JsonView::Bool(*value),
            Self::Number(value) => JsonView::NumberBits(value.to_bits()),
            Self::String(value) => JsonView::String(value.as_str()),
            Self::Array(values) => JsonView::Array(values),
            Self::Object(entries) => JsonView::Object(entries),
        }
    }
}

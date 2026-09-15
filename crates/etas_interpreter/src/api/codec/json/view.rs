use crate::value::HostJsonSupportValue;
use etas_host::HostJsonValue;

pub(in crate::api::codec) enum JsonView<'a, T> {
    Null,
    Bool(bool),
    NumberBits(u64),
    String(&'a str),
    Array(&'a [T]),
    Object(&'a [(String, T)]),
}

pub(in crate::api::codec) trait JsonSource: Sized {
    fn view(&self) -> JsonView<'_, Self>;
}

impl JsonSource for HostJsonSupportValue {
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
    fn view(&self) -> JsonView<'_, Self> {
        match self {
            Self::Null => JsonView::Null,
            Self::Bool(value) => JsonView::Bool(*value),
            Self::Number(value) => JsonView::NumberBits(value.to_bits()),
            Self::String(value) => JsonView::String(value),
            Self::Array(values) => JsonView::Array(values),
            Self::Object(entries) => JsonView::Object(entries),
        }
    }
}

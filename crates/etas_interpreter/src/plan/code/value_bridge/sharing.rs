use etas_types::{TrustWrapper, TypeId};

use crate::value::{InterpValue, StringValue};

#[derive(PartialEq, Eq, Hash)]
pub(super) struct Key {
    pub from: TypeId,
    pub to: TypeId,
    storage: *const (),
    kind: Kind,
}

#[derive(PartialEq, Eq, Hash)]
enum Kind {
    Nominal(TypeId),
    Trust(TrustWrapper),
    Variant(StringValue),
    Some,
    Tuple,
    Array,
    List(usize),
    Slice(usize, usize),
    Record,
    Map,
    Set,
}

pub(super) struct SharedConversion {
    // Keep the source allocation alive for the entire cache entry. Pointer
    // reuse cannot make a later value hit a different source's conversion.
    pub _source: InterpValue,
    pub target: InterpValue,
}

pub(super) fn key(from: TypeId, to: TypeId, value: &InterpValue) -> Option<Key> {
    let (storage, kind) = match value {
        InterpValue::Nominal { ty, value } => {
            (value.shared_capture_identity()?, Kind::Nominal(*ty))
        }
        InterpValue::Trust { wrapper, value } => {
            (value.shared_capture_identity()?, Kind::Trust(*wrapper))
        }
        InterpValue::Variant { name, fields } => (
            fields.shared_capture_identity()?,
            Kind::Variant(name.clone()),
        ),
        InterpValue::OptionSome(value) => (value.shared_capture_identity()?, Kind::Some),
        InterpValue::Tuple(values) => (values.shared_capture_identity()?, Kind::Tuple),
        InterpValue::Array(values) => (values.shared_capture_identity()?, Kind::Array),
        InterpValue::List(values) => (values.shared_capture_identity()?, Kind::List(values.len())),
        InterpValue::Slice(values) => {
            let (storage, start, end) = values.shared_capture_identity()?;
            (storage, Kind::Slice(start, end))
        }
        InterpValue::Record(values) => (values.shared_capture_identity()?, Kind::Record),
        InterpValue::Map(values) => (values.shared_capture_identity()?, Kind::Map),
        InterpValue::Set(values) => (values.shared_capture_identity()?, Kind::Set),
        _ => return None,
    };
    Some(Key {
        from,
        to,
        storage,
        kind,
    })
}

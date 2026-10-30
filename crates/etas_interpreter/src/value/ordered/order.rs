use crate::{
    orchestration::ValueSnapshot,
    value::{InterpValue, NumericValue, StringValue},
};
use std::cmp::Ordering;

enum Key<'a> {
    Number(NumericValue),
    String(&'a StringValue),
    Bool(bool),
}

fn compare(left: Key<'_>, right: Key<'_>) -> Result<Ordering, String> {
    let order = match (left, right) {
        (Key::Number(left), Key::Number(right)) => left
            .partial_cmp_same(right)
            .map_err(|error| format!("invalid collection comparison: {error:?}"))?,
        (Key::String(left), Key::String(right)) => Some(left.cmp(right)),
        (Key::Bool(left), Key::Bool(right)) => Some(left.cmp(&right)),
        _ => None,
    };
    order.ok_or_else(invalid_key)
}

fn invalid_key() -> String {
    "collection keys have no default total order; provide a pure comparison flow".into()
}

pub(crate) fn default_order(left: &InterpValue, right: &InterpValue) -> Result<Ordering, String> {
    fn key(value: &InterpValue) -> Result<Key<'_>, String> {
        match value {
            InterpValue::Number(value) => Ok(Key::Number(*value)),
            InterpValue::String(value) => Ok(Key::String(value)),
            InterpValue::Bool(value) => Ok(Key::Bool(*value)),
            _ => Err(invalid_key()),
        }
    }
    compare(key(left)?, key(right)?)
}

pub(crate) fn snapshot_order(
    left: &ValueSnapshot,
    right: &ValueSnapshot,
) -> Result<Ordering, String> {
    fn key(value: &ValueSnapshot) -> Result<Key<'_>, String> {
        match value {
            ValueSnapshot::Number(value) => Ok(Key::Number(*value)),
            ValueSnapshot::String(value) => Ok(Key::String(value)),
            ValueSnapshot::Bool(value) => Ok(Key::Bool(*value)),
            _ => Err(invalid_key()),
        }
    }
    compare(key(left)?, key(right)?)
}

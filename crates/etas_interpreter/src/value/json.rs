use std::hash::{Hash, Hasher};

use super::{
    HostJsonSupportValue as Json,
    comparison::{Comparison, CursorStep, EqualityCursor, compare},
};

enum Children<'a> {
    Array(std::slice::Iter<'a, Json>),
    Object(std::slice::Iter<'a, (String, Json)>),
}

impl<'a> Children<'a> {
    fn next(&mut self) -> Option<(Option<&'a str>, &'a Json)> {
        match self {
            Self::Array(values) => values.next().map(|v| (None, v)),
            Self::Object(fields) => fields.next().map(|(k, v)| (Some(k.as_str()), v)),
        }
    }
}

struct Cursor<'a>(Children<'a>, Children<'a>);

pub(super) fn equal(a: &Json, b: &Json) -> bool {
    compare(node(a, b))
}

fn node<'a>(a: &'a Json, b: &'a Json) -> Comparison<Cursor<'a>> {
    let equal = match (a, b) {
        (Json::Null, Json::Null) => true,
        (Json::Bool(a), Json::Bool(b)) => a == b,
        (Json::NumberBits(a), Json::NumberBits(b)) => a == b,
        (Json::String(a), Json::String(b)) => a == b,
        (Json::Array(a), Json::Array(b)) if a.len() == b.len() => {
            return Comparison::Pending(Cursor(
                Children::Array(a.iter()),
                Children::Array(b.iter()),
            ));
        }
        (Json::Object(a), Json::Object(b)) if a.len() == b.len() => {
            return Comparison::Pending(Cursor(
                Children::Object(a.iter()),
                Children::Object(b.iter()),
            ));
        }
        _ => false,
    };
    Comparison::Ready(equal)
}

impl<'a> EqualityCursor for Cursor<'a> {
    fn advance(&mut self, previous: Option<bool>) -> CursorStep<Self> {
        if previous == Some(false) {
            return CursorStep::Complete(false);
        }
        match (self.0.next(), self.1.next()) {
            (Some((ak, av)), Some((bk, bv))) if ak == bk => CursorStep::Child(node(av, bv)),
            (None, None) => CursorStep::Complete(true),
            _ => CursorStep::Complete(false),
        }
    }
}

/// Encode structural tags, arities, labels and scalar payloads directly into
/// the keyed hasher. No serialized JSON, cloned payload or recursive traversal.
pub(crate) fn hash(value: &Json, state: &mut impl Hasher) {
    let Some(mut current) = hash_node(value, state) else {
        return;
    };
    let mut parents = Vec::new();
    loop {
        if let Some((name, child)) = current.next() {
            if let Some(name) = name {
                name.hash(state);
            }
            if let Some(children) = hash_node(child, state) {
                parents.push(current);
                current = children;
            }
        } else if let Some(parent) = parents.pop() {
            current = parent;
        } else {
            return;
        }
    }
}

fn hash_node<'a>(value: &'a Json, state: &mut impl Hasher) -> Option<Children<'a>> {
    std::mem::discriminant(value).hash(state);
    match value {
        Json::Null => {}
        Json::Bool(v) => v.hash(state),
        Json::NumberBits(v) => v.hash(state),
        Json::String(v) => v.hash(state),
        Json::Array(values) => {
            values.len().hash(state);
            return Some(Children::Array(values.iter()));
        }
        Json::Object(fields) => {
            fields.len().hash(state);
            return Some(Children::Object(fields.iter()));
        }
    }
    None
}

use std::{fmt, ops::Deref, rc::Rc};

use crate::value::HostJsonSupportValue as Json;

// These backings expose no mutation. Checkpoint capture and ordinary value reads
// share them; host/codec ingress transfers already-validated owned child buffers.
macro_rules! storage {
    ($name:ident, $node:ident, $element:ty, $cursor:ident) => {
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name(Rc<$node>);

        #[derive(PartialEq, Eq)]
        struct $node(Vec<$element>);

        impl $name {
            fn into_unique(self) -> Option<Vec<$element>> {
                Rc::try_unwrap(self.0)
                    .ok()
                    .map(|mut node| std::mem::take(&mut node.0))
            }
        }

        impl From<Vec<$element>> for $name {
            fn from(values: Vec<$element>) -> Self {
                Self(Rc::new($node(values)))
            }
        }

        impl FromIterator<$element> for $name {
            fn from_iter<T: IntoIterator<Item = $element>>(iter: T) -> Self {
                Self::from(iter.into_iter().collect::<Vec<_>>())
            }
        }

        impl Deref for $name {
            type Target = [$element];
            fn deref(&self) -> &Self::Target {
                &self.0.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Debug::fmt(&**self, f)
            }
        }

        impl<'a> IntoIterator for &'a $name {
            type Item = &'a $element;
            type IntoIter = std::slice::Iter<'a, $element>;
            fn into_iter(self) -> Self::IntoIter {
                self.iter()
            }
        }

        impl Drop for $node {
            fn drop(&mut self) {
                release(Cursor::$cursor(std::mem::take(&mut self.0).into_iter()));
            }
        }
    };
}

storage!(JsonArray, ArrayNode, Json, Array);
storage!(JsonObject, ObjectNode, (String, Json), Object);

enum Cursor {
    Array(std::vec::IntoIter<Json>),
    Object(std::vec::IntoIter<(String, Json)>),
}

impl Cursor {
    fn next(&mut self) -> Option<Json> {
        match self {
            Self::Array(values) => values.next(),
            Self::Object(fields) => fields.next().map(|(_, value)| value),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Array(values) => values.len() == 0,
            Self::Object(fields) => fields.len() == 0,
        }
    }
}

fn release(mut current: Cursor) {
    let mut parents = Vec::new();
    loop {
        if let Some(child) = current.next() {
            let children = match child {
                Json::Array(values) => values.into_unique().map(|v| Cursor::Array(v.into_iter())),
                Json::Object(fields) => fields.into_unique().map(|v| Cursor::Object(v.into_iter())),
                _ => None,
            };
            if let Some(children) = children {
                if !current.is_empty() {
                    parents.push(current);
                }
                current = children;
            }
        } else if let Some(parent) = parents.pop() {
            current = parent;
        } else {
            return;
        }
    }
}

#[cfg(test)]
mod tests;

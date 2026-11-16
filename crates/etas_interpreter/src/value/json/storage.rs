use std::{fmt, ops::Deref, rc::Rc};

use crate::value::HostJsonSupportValue as Json;

// These backings expose no mutation. Checkpoint capture and ordinary value reads
// share them; host/codec ingress transfers already-validated owned child buffers.
macro_rules! storage {
    ($name:ident, $node:ident, $element:ty, $cursor:ident $(, $field:ident: $ty:ty = $initialize:expr)?) => {
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name(Rc<$node>);

        #[derive(PartialEq, Eq)]
        struct $node {
            values: Vec<$element>,
            $($field: $ty,)?
        }

        impl $name {
            fn into_unique(self) -> Option<Vec<$element>> {
                Rc::try_unwrap(self.0)
                    .ok()
                    .map(|mut node| std::mem::take(&mut node.values))
            }
        }

        impl From<Vec<$element>> for $name {
            fn from(values: Vec<$element>) -> Self {
                $(let $field = ($initialize)(&values);)?
                Self(Rc::new($node { values, $($field,)? }))
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
                &self.0.values
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
                release(Cursor::$cursor(std::mem::take(&mut self.values).into_iter()));
            }
        }
    };
}

storage!(JsonArray, ArrayNode, Json, Array);
storage!(JsonObject, ObjectNode, (String, Json), Object,
    order: Option<Box<[usize]>> = canonical_order);

fn canonical_order(fields: &[(String, Json)]) -> Option<Box<[usize]>> {
    if fields.windows(2).all(|pair| pair[0].0 <= pair[1].0) {
        return None;
    }
    let mut order = (0..fields.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| fields[*left].0.cmp(&fields[*right].0));
    Some(order.into_boxed_slice())
}

pub(crate) struct JsonObjectIter<'a> {
    object: &'a JsonObject,
    next: usize,
}

impl JsonObject {
    pub(crate) fn canonical_iter(&self) -> JsonObjectIter<'_> {
        JsonObjectIter {
            object: self,
            next: 0,
        }
    }
}

impl<'a> Iterator for JsonObjectIter<'a> {
    type Item = &'a (String, Json);
    fn next(&mut self) -> Option<Self::Item> {
        let index = match &self.object.0.order {
            Some(order) => *order.get(self.next)?,
            None => self.next,
        };
        let value = self.object.0.values.get(index)?;
        self.next += 1;
        Some(value)
    }
}

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

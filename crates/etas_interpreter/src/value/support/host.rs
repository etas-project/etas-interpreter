use super::HostSupportValue as H;
use std::{fmt, ops::Deref, rc::Rc};

// Host payloads are immutable after checked ingress. Clone/capture share backing;
// only the final owner detaches children for iterative destruction.
macro_rules! storage {
    ($name:ident, $node:ident, $element:ty, $cursor:ident) => {
        #[derive(Clone)]
        pub struct $name(Rc<$node>);
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

storage!(HostValues, ValuesNode, H, Values);
storage!(HostFields, FieldsNode, (String, H), Fields);
storage!(HostPairs, PairsNode, (H, H), Pairs);

enum Cursor {
    Values(std::vec::IntoIter<H>),
    Fields(std::vec::IntoIter<(String, H)>),
    Pairs(std::vec::IntoIter<(H, H)>),
    Pending(H),
}

impl Cursor {
    fn is_empty(&self) -> bool {
        match self {
            Self::Values(values) => values.len() == 0,
            Self::Fields(values) => values.len() == 0,
            Self::Pairs(values) => values.len() == 0,
            Self::Pending(_) => false,
        }
    }
}

fn release(mut current: Cursor) {
    let mut parents = Vec::new();
    loop {
        let next = match &mut current {
            Cursor::Values(values) => values.next(),
            Cursor::Fields(values) => values.next().map(|(_, value)| value),
            Cursor::Pairs(values) => values.next().map(|(key, value)| {
                parents.push(Cursor::Pending(value));
                key
            }),
            Cursor::Pending(value) => Some(std::mem::replace(value, H::Unit)),
        };
        // Pending is a single owned value, unlike the iterator cursors.
        if matches!(current, Cursor::Pending(_)) {
            current = Cursor::Values(Vec::new().into_iter());
        }
        if let Some(value) = next {
            let children = match value {
                H::List(values) | H::Variant { fields: values, .. } => {
                    values.into_unique().map(|v| Cursor::Values(v.into_iter()))
                }
                H::Map(values) => values.into_unique().map(|v| Cursor::Pairs(v.into_iter())),
                H::Record(values) => values.into_unique().map(|v| Cursor::Fields(v.into_iter())),
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

pub(super) fn equal(left: &H, right: &H) -> bool {
    let mut pending = Vec::new();
    let mut root = Some((left, right));
    while let Some((left, right)) = root.take().or_else(|| pending.pop()) {
        if std::ptr::eq(left, right) {
            continue;
        }
        match (left, right) {
            (H::Unit, H::Unit) => {}
            (H::Bool(a), H::Bool(b)) if a == b => {}
            (H::Int(a), H::Int(b)) | (H::UInt(a), H::UInt(b)) | (H::String(a), H::String(b))
                if a == b => {}
            (H::FloatBits(a), H::FloatBits(b)) if a == b => {}
            (H::Bytes(a), H::Bytes(b)) if a == b => {}
            (H::Json(a), H::Json(b)) if a == b => {}
            (H::List(a), H::List(b)) if a.len() == b.len() => {
                if !Rc::ptr_eq(&a.0, &b.0) {
                    pending.extend(a.iter().zip(b.iter()));
                }
            }
            (
                H::Variant {
                    name: a,
                    fields: av,
                },
                H::Variant {
                    name: b,
                    fields: bv,
                },
            ) if a == b && av.len() == bv.len() => {
                if !Rc::ptr_eq(&av.0, &bv.0) {
                    pending.extend(av.iter().zip(bv.iter()));
                }
            }
            (H::Map(a), H::Map(b)) if a.len() == b.len() => {
                if !Rc::ptr_eq(&a.0, &b.0) {
                    for ((ak, av), (bk, bv)) in a.iter().zip(b.iter()) {
                        pending.push((ak, bk));
                        pending.push((av, bv));
                    }
                }
            }
            (H::Record(a), H::Record(b)) if a.len() == b.len() => {
                if !Rc::ptr_eq(&a.0, &b.0) {
                    for ((ak, av), (bk, bv)) in a.iter().zip(b.iter()) {
                        if ak != bk {
                            return false;
                        }
                        pending.push((av, bv));
                    }
                }
            }
            _ => return false,
        }
    }
    true
}

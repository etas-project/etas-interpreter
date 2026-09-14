use std::{fmt, ops::Deref, rc::Rc};

/// Immutable byte values share their backing; owned boundaries consume or copy it.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BytesValue(Rc<Vec<u8>>);

impl BytesValue {
    pub fn into_vec(self) -> Vec<u8> {
        Rc::unwrap_or_clone(self.0)
    }
}

impl Deref for BytesValue {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for BytesValue {
    fn from(value: Vec<u8>) -> Self {
        Self(Rc::new(value))
    }
}

impl AsRef<[u8]> for BytesValue {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl serde::Serialize for BytesValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&**self, serializer)
    }
}

impl fmt::Debug for BytesValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

#[cfg(test)]
mod tests;

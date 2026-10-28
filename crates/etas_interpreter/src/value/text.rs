use std::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    ops::Deref,
    rc::Rc,
};

mod concat;
use concat::Repr;
pub(crate) use concat::TextCapacityExceeded;

/// Immutable aliases share text; consuming operations may reuse a unique buffer.
#[derive(Clone, Default)]
pub struct StringValue(Repr);

impl StringValue {
    pub fn into_string(self) -> String {
        let length = self.len();
        concat::into_string(self.into_repr(), length)
    }

    pub fn push_str(&mut self, suffix: &str) {
        if suffix.is_empty() {
            return;
        }
        self.collapse_materialized();
        match &mut self.0 {
            Repr::Flat(text) => Rc::make_mut(text).push_str(suffix),
            Repr::Empty => *self = Self::from(suffix),
            Repr::Concat(_) => {
                let mut text = std::mem::take(self).into_string();
                text.push_str(suffix);
                *self = Self::from(text);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Share an unchanged prefix while the caller's old slot or aliases remain
    /// readable. Assignment still commits only after all RHS evaluation succeeds.
    pub(crate) fn concat(mut self, suffix: Self) -> Result<Self, TextCapacityExceeded> {
        let length = self
            .len()
            .checked_add(suffix.len())
            .filter(|length| *length <= isize::MAX as usize)
            .ok_or(TextCapacityExceeded)?;
        if suffix.is_empty() {
            return Ok(self);
        }
        self.collapse_materialized();
        if let Repr::Flat(text) = &mut self.0
            && let Some(text) = Rc::get_mut(text)
        {
            text.push_str(&suffix);
            return Ok(self);
        }
        if self.is_empty() {
            return Ok(suffix);
        }
        // One prefix edge per node keeps both materialization and release iterative.
        let suffix = concat::into_flat(suffix.into_repr());
        Ok(Self(Repr::Concat(Rc::new(concat::Concat {
            prefix: self,
            suffix,
            length,
            materialized: Default::default(),
        }))))
    }

    fn collapse_materialized(&mut self) {
        if let Repr::Concat(node) = &self.0
            && let Some(text) = node.materialized.get()
        {
            self.0 = Repr::Flat(text.clone());
        }
    }

    fn into_repr(mut self) -> Repr {
        std::mem::take(&mut self.0)
    }

    fn same_backing(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Repr::Empty, Repr::Empty) => true,
            (Repr::Flat(left), Repr::Flat(right)) => Rc::ptr_eq(left, right),
            (Repr::Concat(left), Repr::Concat(right)) => Rc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl Drop for StringValue {
    fn drop(&mut self) {
        concat::release(std::mem::take(&mut self.0));
    }
}

impl Deref for StringValue {
    type Target = String;

    fn deref(&self) -> &String {
        self.0.text()
    }
}

impl From<String> for StringValue {
    fn from(value: String) -> Self {
        Self(Repr::Flat(Rc::new(value)))
    }
}

impl From<&str> for StringValue {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

impl From<StringValue> for String {
    fn from(value: StringValue) -> Self {
        value.into_string()
    }
}

impl FromIterator<char> for StringValue {
    fn from_iter<T: IntoIterator<Item = char>>(iter: T) -> Self {
        String::from_iter(iter).into()
    }
}

impl AsRef<str> for StringValue {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq for StringValue {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && (self.is_empty() || self.same_backing(other) || self.as_str() == other.as_str())
    }
}
impl Eq for StringValue {}
impl PartialOrd for StringValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for StringValue {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.same_backing(other) {
            Ordering::Equal
        } else {
            self.as_str().cmp(other.as_str())
        }
    }
}
impl Hash for StringValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl PartialEq<str> for StringValue {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for StringValue {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for StringValue {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<StringValue> for String {
    fn eq(&self, other: &StringValue) -> bool {
        self.as_str() == other.as_str()
    }
}

impl fmt::Debug for StringValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for StringValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for StringValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;

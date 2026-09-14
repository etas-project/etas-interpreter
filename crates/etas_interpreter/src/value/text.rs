use std::{fmt, ops::Deref, rc::Rc};

/// Immutable aliases share text; consuming operations may reuse a unique buffer.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StringValue(Rc<String>);

impl StringValue {
    pub fn into_string(self) -> String {
        Rc::unwrap_or_clone(self.0)
    }

    pub fn push_str(&mut self, suffix: &str) {
        Rc::make_mut(&mut self.0).push_str(suffix);
    }
}

impl Deref for StringValue {
    type Target = String;

    fn deref(&self) -> &String {
        &self.0
    }
}

impl From<String> for StringValue {
    fn from(value: String) -> Self {
        Self(Rc::new(value))
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

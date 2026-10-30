use std::{
    cmp::Ordering,
    hash::{Hash, Hasher},
    ops::Deref,
    rc::Rc,
};

mod chunks;
mod concat;
mod formatting;
mod ranges;
mod read;
pub use chunks::TextChunks;
use concat::Repr;
pub(crate) use concat::TextCapacityExceeded;
pub(crate) use ranges::{TextRangeError, TextRangeProjector};
pub(crate) use read::hash_text;

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

    pub fn chunks(&self) -> TextChunks<'_> {
        TextChunks::new(self)
    }

    /// Share an unchanged prefix while the caller's old slot or aliases remain
    /// readable. Assignment still commits only after all RHS evaluation succeeds.
    pub(crate) fn concat(mut self, mut suffix: Self) -> Result<Self, TextCapacityExceeded> {
        let length = self
            .len()
            .checked_add(suffix.len())
            .filter(|length| *length <= isize::MAX as usize)
            .ok_or(TextCapacityExceeded)?;
        if suffix.is_empty() {
            return Ok(self);
        }
        self.collapse_materialized();
        suffix.collapse_materialized();
        if self.append_if_spare(&suffix, "", length) {
            return Ok(self);
        }
        if self.is_empty() {
            return Ok(suffix);
        }
        let leaves = self
            .0
            .leaves()
            .checked_add(suffix.0.leaves())
            .ok_or(TextCapacityExceeded)?;
        Ok(Self(Repr::Concat(Rc::new(concat::Concat {
            prefix: self,
            suffix,
            length,
            leaves,
            materialized: Default::default(),
        }))))
    }

    pub(crate) fn concat_separated(
        mut self,
        mut suffix: Self,
        separator: &str,
    ) -> Result<Self, TextCapacityExceeded> {
        let length = self
            .len()
            .checked_add(separator.len())
            .and_then(|length| length.checked_add(suffix.len()))
            .filter(|length| *length <= isize::MAX as usize)
            .ok_or(TextCapacityExceeded)?;
        self.collapse_materialized();
        suffix.collapse_materialized();
        if self.append_if_spare(&suffix, separator, length) {
            return Ok(self);
        }
        self.concat(separator.into())?.concat(suffix)
    }

    fn append_if_spare(&mut self, suffix: &Self, separator: &str, length: usize) -> bool {
        if let Repr::Flat(text) = &mut self.0
            && text.capacity() >= length
            && let Some(text) = Rc::get_mut(text)
        {
            text.push_str(separator);
            for chunk in chunks::TextLeaves::new(&suffix.0) {
                text.push_str(chunk);
            }
            return true;
        }
        false
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

impl etas_builtin::text::input::TextInput for StringValue {
    type Chunks<'a> = TextChunks<'a>;

    fn byte_len(&self) -> usize {
        self.len()
    }

    fn contiguous(&self) -> Option<&str> {
        match &self.0 {
            Repr::Empty => Some(""),
            Repr::Flat(text) => Some(text.as_str()),
            Repr::Concat(node) => node.materialized.get().map(|text| text.as_str()),
        }
    }

    fn chunks(&self) -> Self::Chunks<'_> {
        StringValue::chunks(self)
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
        self.len() == other.len() && self.compare_borrowed(other).is_eq()
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
        self.compare_borrowed(other)
    }
}
impl Hash for StringValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Preserve public Rust str hashing. Runtime bucket partitions use the
        // layout-independent streaming protocol in hash_membership instead.
        self.as_str().hash(state);
    }
}

impl PartialEq<str> for StringValue {
    fn eq(&self, other: &str) -> bool {
        self.equals_str(other)
    }
}

impl PartialEq<&str> for StringValue {
    fn eq(&self, other: &&str) -> bool {
        self.equals_str(other)
    }
}

impl PartialEq<String> for StringValue {
    fn eq(&self, other: &String) -> bool {
        self.equals_str(other.as_str())
    }
}

impl PartialEq<StringValue> for String {
    fn eq(&self, other: &StringValue) -> bool {
        other.equals_str(self.as_str())
    }
}

#[cfg(test)]
mod read_tests;
#[cfg(test)]
mod suffix_tests;
#[cfg(test)]
mod tests;

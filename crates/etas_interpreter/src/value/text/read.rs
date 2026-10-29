use std::{
    cmp::Ordering,
    hash::{Hash, Hasher},
};

use etas_builtin::text::input::TextInput;

use super::StringValue;

impl StringValue {
    pub(super) fn compare_borrowed(&self, other: &Self) -> Ordering {
        if self.same_backing(other) {
            return Ordering::Equal;
        }
        if self.is_empty() || other.is_empty() {
            return self.len().cmp(&other.len());
        }
        if let (Some(left), Some(right)) = (self.contiguous(), other.contiguous()) {
            return left.cmp(right);
        }
        compare_parts(self.chunks(), other.chunks())
    }

    pub(super) fn equals_str(&self, other: &str) -> bool {
        if self.len() != other.len() {
            return false;
        }
        if let Some(text) = self.contiguous() {
            return text == other;
        }
        compare_parts(self.chunks(), std::iter::once(other)).is_eq()
    }

    /// Bucket hashes are process-local and never persisted. Fixed logical byte
    /// blocks make the Hasher call sequence independent of physical chunks;
    /// adjacent Hasher::write calls are not generally interchangeable.
    pub(crate) fn hash_membership(&self, state: &mut impl Hasher) {
        hash_text(self, state);
    }
}

pub(crate) fn hash_text<T: TextInput + ?Sized>(text: &T, state: &mut impl Hasher) {
    const BLOCK: usize = 64;
    text.byte_len().hash(state);
    let mut pending = [0u8; BLOCK];
    let mut used = 0;
    for part in text.chunks() {
        let mut bytes = part.as_bytes();
        if used != 0 {
            let take = (BLOCK - used).min(bytes.len());
            pending[used..used + take].copy_from_slice(&bytes[..take]);
            used += take;
            bytes = &bytes[take..];
            if used != BLOCK {
                continue;
            }
            state.write(&pending);
        }
        let complete = bytes.len() / BLOCK * BLOCK;
        for block in bytes[..complete].chunks_exact(BLOCK) {
            state.write(block);
        }
        let tail = &bytes[complete..];
        pending[..tail.len()].copy_from_slice(tail);
        used = tail.len();
    }
    if used != 0 {
        state.write(&pending[..used]);
    }
}

fn compare_parts<'a, 'b>(
    left: impl Iterator<Item = &'a str>,
    right: impl Iterator<Item = &'b str>,
) -> Ordering {
    let mut left = left.filter(|part| !part.is_empty()).map(str::as_bytes);
    let mut right = right.filter(|part| !part.is_empty()).map(str::as_bytes);
    let mut a = left.next();
    let mut b = right.next();
    loop {
        match (a, b) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let length = x.len().min(y.len());
                let order = x[..length].cmp(&y[..length]);
                if !order.is_eq() {
                    return order;
                }
                a = if length == x.len() {
                    left.next()
                } else {
                    Some(&x[length..])
                };
                b = if length == y.len() {
                    right.next()
                } else {
                    Some(&y[length..])
                };
            }
        }
    }
}

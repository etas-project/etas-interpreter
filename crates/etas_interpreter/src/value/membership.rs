use std::{
    collections::HashMap,
    hash::{BuildHasher, Hasher},
};

mod runtime;
pub(crate) mod structure;
#[cfg(test)]
mod tests;

/// A partition only narrows equality candidates; it is never equality evidence.
/// Equal values must share a partition. Collisions always use the value's Eq.
pub(crate) trait MembershipValue: PartialEq {
    fn hash_partition(&self, state: &mut (impl Hasher + Clone));
    fn member_eq(&self, other: &Self) -> bool {
        self == other
    }
}

#[derive(Clone, Default)]
pub(crate) struct MembershipIndex {
    buckets: HashMap<u64, Bucket>,
    #[cfg(test)]
    comparisons: std::cell::Cell<usize>,
}

#[derive(Clone)]
struct Bucket {
    first: usize,
    collisions: Vec<usize>,
}

impl MembershipIndex {
    pub(crate) fn fingerprint(&self, value: &impl MembershipValue) -> u64 {
        let mut state = self.buckets.hasher().build_hasher();
        value.hash_partition(&mut state);
        state.finish()
    }

    pub(crate) fn contains<T: MembershipValue>(&self, values: &[T], value: &T, hash: u64) -> bool {
        self.buckets.get(&hash).is_some_and(|bucket| {
            let matches = |index: usize| {
                #[cfg(test)]
                self.comparisons.set(self.comparisons.get() + 1);
                values[index].member_eq(value)
            };
            matches(bucket.first) || bucket.collisions.iter().copied().any(matches)
        })
    }

    #[cfg(test)]
    pub(crate) fn comparison_count(&self) -> usize {
        self.comparisons.get()
    }

    pub(crate) fn insert(&mut self, hash: u64, index: usize) {
        self.buckets
            .entry(hash)
            .and_modify(|bucket| bucket.collisions.push(index))
            .or_insert_with(|| Bucket {
                first: index,
                collisions: Vec::new(),
            });
    }

    pub(crate) fn require_unique<T: MembershipValue>(values: &[T]) -> Result<Self, String> {
        let mut index = Self::default();
        for (position, value) in values.iter().enumerate() {
            let hash = index.fingerprint(value);
            if index.contains(values, value, hash) {
                return Err(format!("duplicate set element at index {position}"));
            }
            index.insert(hash, position);
        }
        Ok(index)
    }
}

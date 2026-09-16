use std::{cell::OnceCell, fmt, rc::Rc};

use super::{InterpValue, membership::MembershipIndex};

#[derive(Clone)]
pub struct MapValue(Rc<MapStorage>);

#[derive(Clone)]
struct MapStorage {
    entries: Vec<(InterpValue, InterpValue)>,
    // Derived and never serialized. COW value-only edits can share this index.
    index: OnceCell<Rc<MembershipIndex>>,
}

impl MapStorage {
    fn index(&self) -> &MembershipIndex {
        self.index.get_or_init(|| {
            let mut index = MembershipIndex::with_capacity(self.entries.len());
            for (position, (key, _)) in self.entries.iter().enumerate() {
                index.insert(index.fingerprint(key), position);
            }
            Rc::new(index)
        })
    }

    fn position(&self, key: &InterpValue) -> Option<usize> {
        let index = self.index();
        index.position(index.fingerprint(key), |position| {
            #[cfg(test)]
            tests::record_comparison();
            self.entries[position].0 == *key
        })
    }
}

impl MapValue {
    pub(crate) fn shared_capture_identity(&self) -> Option<*const ()> {
        (Rc::strong_count(&self.0) > 1).then(|| Rc::as_ptr(&self.0).cast())
    }

    pub fn new(entries: Vec<(InterpValue, InterpValue)>) -> Self {
        Self(Rc::new(MapStorage {
            entries,
            index: OnceCell::new(),
        }))
    }

    pub(super) fn into_unique_values(self) -> Option<Vec<(InterpValue, InterpValue)>> {
        Rc::try_unwrap(self.0).ok().map(|data| data.entries)
    }

    pub fn contains_key(&self, key: &InterpValue) -> bool {
        self.0.position(key).is_some()
    }

    pub fn get(&self, key: &InterpValue) -> Option<InterpValue> {
        self.get_ref(key).cloned()
    }

    pub(crate) fn get_ref(&self, key: &InterpValue) -> Option<&InterpValue> {
        let data = &self.0;
        data.position(key).map(|position| &data.entries[position].1)
    }

    /// Only values are writable through this reference; key positions remain valid.
    pub(crate) fn value_mut(&mut self, key: &InterpValue) -> Option<&mut InterpValue> {
        let position = self.0.position(key)?;
        Some(&mut Rc::make_mut(&mut self.0).entries[position].1)
    }

    pub(crate) fn insert(&mut self, key: InterpValue, value: InterpValue) {
        let existing = self.0.position(&key);
        if existing.is_none() && Rc::strong_count(&self.0) > 1 {
            let data = &self.0;
            let mut entries = Vec::with_capacity(data.entries.len() + 1);
            entries.extend(data.entries.iter().cloned());
            let next = MapStorage {
                entries,
                index: data.index.clone(),
            };
            self.0 = Rc::new(next);
        } else {
            self.make_unique();
        }
        let data = Rc::make_mut(&mut self.0);
        if let Some(position) = existing {
            data.entries[position].1 = value;
        } else {
            let hash = data.index().fingerprint(&key);
            let position = data.entries.len();
            data.entries.push((key, value));
            if let Some(index) = data.index.get_mut() {
                Rc::make_mut(index).insert(hash, position);
            }
        }
    }

    pub fn borrow(&self) -> &Vec<(InterpValue, InterpValue)> {
        &self.0.entries
    }

    /// Arbitrary key/reorder edits invalidate only this COW version's cache.
    pub fn borrow_mut(&mut self) -> &mut Vec<(InterpValue, InterpValue)> {
        let data = Rc::make_mut(&mut self.0);
        data.index.take();
        &mut data.entries
    }

    pub fn snapshot(&self) -> Vec<(InterpValue, InterpValue)> {
        self.borrow().clone()
    }

    pub fn into_values(self) -> Vec<(InterpValue, InterpValue)> {
        match Rc::try_unwrap(self.0) {
            Ok(data) => data.entries,
            Err(shared) => shared.entries.clone(),
        }
    }

    pub fn make_unique(&mut self) {
        Rc::make_mut(&mut self.0);
    }
}

impl fmt::Debug for MapValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("MapValue").field(&self.borrow()).finish()
    }
}

impl PartialEq for MapValue {
    fn eq(&self, other: &Self) -> bool {
        *self.borrow() == *other.borrow()
    }
}
impl Eq for MapValue {}

impl From<Vec<(InterpValue, InterpValue)>> for MapValue {
    fn from(entries: Vec<(InterpValue, InterpValue)>) -> Self {
        Self::new(entries)
    }
}

#[cfg(test)]
mod tests;

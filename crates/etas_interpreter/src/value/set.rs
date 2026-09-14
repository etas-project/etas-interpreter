use std::{fmt, rc::Rc};

use super::{InterpValue, membership::MembershipIndex};

#[derive(Clone)]
pub struct SetValue(Rc<SetStorage>);

#[derive(Clone, Default)]
struct SetStorage {
    values: Vec<InterpValue>,
    index: MembershipIndex,
}

impl SetValue {
    pub fn new(mut values: Vec<InterpValue>) -> Self {
        let mut index = MembershipIndex::default();
        let mut retained = 0;
        for position in 0..values.len() {
            let hash = index.fingerprint(&values[position]);
            if !index.contains(&values, &values[position], hash) {
                values.swap(retained, position);
                index.insert(hash, retained);
                retained += 1;
            }
        }
        values.truncate(retained);
        Self(Rc::new(SetStorage { values, index }))
    }

    pub(crate) fn from_unique(values: Vec<InterpValue>) -> Result<Self, String> {
        let index = MembershipIndex::require_unique(&values)?;
        Ok(Self(Rc::new(SetStorage { values, index })))
    }

    pub fn borrow(&self) -> &[InterpValue] {
        &self.0.values
    }

    pub fn contains(&self, value: &InterpValue) -> bool {
        self.0
            .index
            .contains(&self.0.values, value, self.0.index.fingerprint(value))
    }

    pub fn insert(&mut self, value: InterpValue) -> bool {
        let hash = self.0.index.fingerprint(&value);
        if self.0.index.contains(&self.0.values, &value, hash) {
            return false;
        }
        let storage = Rc::make_mut(&mut self.0);
        storage.index.insert(hash, storage.values.len());
        storage.values.push(value);
        true
    }

    pub fn clear(&mut self) {
        self.0 = Rc::new(SetStorage::default());
    }

    pub fn snapshot(&self) -> Vec<InterpValue> {
        self.0.values.clone()
    }

    pub fn into_values(self) -> Vec<InterpValue> {
        match Rc::try_unwrap(self.0) {
            Ok(storage) => storage.values,
            Err(storage) => storage.values.clone(),
        }
    }

    pub(super) fn into_unique_values(self) -> Option<Vec<InterpValue>> {
        Rc::try_unwrap(self.0).ok().map(|storage| storage.values)
    }
}

impl From<Vec<InterpValue>> for SetValue {
    fn from(values: Vec<InterpValue>) -> Self {
        Self::new(values)
    }
}

impl PartialEq for SetValue {
    fn eq(&self, other: &Self) -> bool {
        self.0.values.len() == other.0.values.len()
            && self.0.values.iter().all(|value| other.contains(value))
    }
}
impl Eq for SetValue {}

impl fmt::Debug for SetValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SetValue").field(&self.borrow()).finish()
    }
}

#[cfg(test)]
mod tests;

use std::{collections::HashMap, hash::Hash};

// Keep the common single-pair case on the stack; larger graphs spill to the
// keyed table without copying captured values.
pub(super) struct Table<K> {
    first: Option<(K, usize)>,
    rest: HashMap<K, usize>,
}

impl<K> Default for Table<K> {
    fn default() -> Self {
        Self {
            first: None,
            rest: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash> Table<K> {
    pub(super) fn get(&self, key: &K) -> Option<usize> {
        if let Some((first, slot)) = &self.first
            && first == key
        {
            Some(*slot)
        } else {
            self.rest.get(key).copied()
        }
    }
    pub(super) fn insert(&mut self, key: K, slot: usize) {
        match &mut self.first {
            None => self.first = Some((key, slot)),
            Some((first, old_slot)) if first == &key => *old_slot = slot,
            _ => {
                self.rest.insert(key, slot);
            }
        }
    }
}

#[derive(Default)]
pub(super) struct States {
    first: u32,
    count: usize,
    rest: Vec<u32>,
}

impl States {
    pub(super) fn push(&mut self, state: u32) -> usize {
        let slot = self.count;
        self.count += 1;
        if slot == 0 {
            self.first = state;
        } else {
            self.rest.push(state);
        }
        slot
    }
    pub(super) fn get(&self, slot: usize) -> u32 {
        if slot == 0 {
            self.first
        } else {
            self.rest[slot - 1]
        }
    }
    pub(super) fn set(&mut self, slot: usize, state: u32) {
        if slot == 0 {
            self.first = state;
        } else {
            self.rest[slot - 1] = state;
        }
    }
    pub(super) fn invalidate_completed(&mut self, active: u32, invalid: u32) {
        if self.count > 0 && self.first != active {
            self.first = invalid;
        }
        for state in &mut self.rest {
            if *state != active {
                *state = invalid;
            }
        }
    }
}

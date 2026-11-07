use std::collections::{BTreeMap, VecDeque};

use super::TaskId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ResourceGuardId(pub(crate) u64);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EffectIdentity {
    Memory,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FrozenSelector {
    pub(crate) region: String,
    pub(crate) path: Vec<String>,
    pub(crate) key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RuntimeResourceKey {
    pub(crate) effect: EffectIdentity,
    pub(crate) selector: FrozenSelector,
}

impl RuntimeResourceKey {
    fn conflicts(&self, other: &Self) -> bool {
        self.effect == other.effect
            && self.selector.region == other.selector.region
            && (self.selector.path.starts_with(&other.selector.path)
                || other.selector.path.starts_with(&self.selector.path))
            && (self.selector.key.is_none()
                || other.selector.key.is_none()
                || self.selector.key == other.selector.key)
    }
}

pub(crate) enum ResourceEvent {
    Acquired(TaskId, usize),
    Waiting(TaskId, ResourceGuardId),
    Released(TaskId, usize),
}

#[derive(Default)]
pub(super) struct ResourceGuards {
    pub(super) holders: BTreeMap<TaskId, Vec<RuntimeResourceKey>>,
    pub(super) waiting: VecDeque<TaskId>,
    pub(super) events: Vec<ResourceEvent>,
}

impl ResourceGuards {
    pub(super) fn acquire(&mut self, task: TaskId, keys: &[RuntimeResourceKey]) -> bool {
        if self.holders.contains_key(&task) {
            return true;
        }
        // Check all permits before taking any. Sorted keys and FIFO admission
        // make multi-resource acquisition independent of source access order.
        if keys.iter().any(|key| {
            self.holders
                .values()
                .flatten()
                .any(|held| key.conflicts(held))
        }) {
            return false;
        }
        self.holders.insert(task, keys.to_vec());
        if !keys.is_empty() {
            self.events.push(ResourceEvent::Acquired(task, keys.len()));
        }
        true
    }

    pub(super) fn release(&mut self, task: TaskId) {
        if let Some(keys) = self.holders.remove(&task)
            && !keys.is_empty()
        {
            self.events.push(ResourceEvent::Released(task, keys.len()));
        }
    }
}

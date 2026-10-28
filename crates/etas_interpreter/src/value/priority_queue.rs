use std::{cmp::Ordering, rc::Rc};

use crate::control::CallTarget;

use super::InterpValue;

#[cfg(test)]
mod tests;

/// Slots retain insertion order; the heap contains only live slot indices.
/// Neither index nor vacant slots cross the checkpoint ABI.
#[derive(Clone, Debug)]
pub struct PriorityQueueValue(Rc<Storage>);

#[derive(Debug)]
struct Storage {
    slots: Vec<Option<(InterpValue, InterpValue)>>,
    heap: Vec<usize>,
    comparator: Option<CallTarget>,
    ready: bool,
}

impl Clone for Storage {
    fn clone(&self) -> Self {
        #[cfg(test)]
        tests::record_clone(self.heap.len(), self.slots.len());
        let mut slots = Vec::with_capacity(self.slots.len() + 1);
        slots.extend(self.slots.iter().cloned());
        let mut heap = Vec::with_capacity(self.heap.len() + 1);
        heap.extend_from_slice(&self.heap);
        Self {
            slots,
            heap,
            comparator: self.comparator.clone(),
            ready: self.ready,
        }
    }
}

impl Drop for Storage {
    fn drop(&mut self) {
        for (priority, value) in std::mem::take(&mut self.slots).into_iter().flatten() {
            super::shared::release_value(priority);
            super::shared::release_value(value);
        }
        if let Some(target) = self.comparator.take() {
            super::shared::release_value(InterpValue::Callable(target));
        }
    }
}

impl PriorityQueueValue {
    pub fn new(comparator: Option<CallTarget>) -> Self {
        Self(Rc::new(Storage {
            slots: Vec::new(),
            heap: Vec::new(),
            comparator,
            ready: true,
        }))
    }

    pub(crate) fn from_entries(
        entries: Vec<(InterpValue, InterpValue)>,
        comparator: Option<CallTarget>,
    ) -> Self {
        let count = entries.len();
        Self(Rc::new(Storage {
            slots: entries.into_iter().map(Some).collect(),
            heap: (0..count).collect(),
            comparator,
            ready: count < 2,
        }))
    }

    pub fn len(&self) -> usize {
        self.0.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.heap.is_empty()
    }

    pub fn iter(&self) -> PriorityQueueIter<'_> {
        PriorityQueueIter {
            slots: self.0.slots.iter(),
            remaining: self.len(),
        }
    }

    pub(crate) fn slot(&self, position: usize) -> Option<&(InterpValue, InterpValue)> {
        self.0.slots.get(position).and_then(Option::as_ref)
    }

    pub(crate) fn slot_count(&self) -> usize {
        self.0.slots.len()
    }

    pub(crate) fn comparator(&self) -> Option<&CallTarget> {
        self.0.comparator.as_ref()
    }

    pub(crate) fn shared_capture_identity(&self) -> Option<*const ()> {
        (Rc::strong_count(&self.0) > 1).then(|| Rc::as_ptr(&self.0).cast())
    }

    pub(super) fn into_unique_values(self) -> Option<PriorityQueueIntoIter> {
        Rc::try_unwrap(self.0)
            .ok()
            .map(|mut storage| PriorityQueueIntoIter {
                slots: std::mem::take(&mut storage.slots).into_iter(),
                pending: None,
                comparator: storage.comparator.take(),
            })
    }

    pub(crate) fn update(self, operation: Operation) -> Update {
        let stage = if self.0.ready {
            Stage::Apply
        } else {
            Stage::Rebuild(self.len() / 2)
        };
        Update {
            queue: self,
            was_pop: operation.entry.is_none(),
            operation: Some(operation),
            popped: None,
            stage,
            rebuilding: None,
            pending: None,
        }
    }
}

pub(super) struct PriorityQueueIntoIter {
    slots: std::vec::IntoIter<Option<(InterpValue, InterpValue)>>,
    pending: Option<InterpValue>,
    comparator: Option<CallTarget>,
}

impl Iterator for PriorityQueueIntoIter {
    type Item = InterpValue;
    fn next(&mut self) -> Option<InterpValue> {
        if let Some(value) = self.pending.take() {
            return Some(value);
        }
        if let Some((priority, value)) = self.slots.find_map(|entry| entry) {
            self.pending = Some(value);
            Some(priority)
        } else {
            self.comparator.take().map(InterpValue::Callable)
        }
    }
}

pub struct PriorityQueueIter<'a> {
    slots: std::slice::Iter<'a, Option<(InterpValue, InterpValue)>>,
    remaining: usize,
}

impl<'a> Iterator for PriorityQueueIter<'a> {
    type Item = &'a (InterpValue, InterpValue);
    fn next(&mut self) -> Option<Self::Item> {
        let value = self.slots.find_map(Option::as_ref)?;
        self.remaining -= 1;
        Some(value)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for PriorityQueueIter<'_> {}

impl From<Vec<(InterpValue, InterpValue)>> for PriorityQueueValue {
    fn from(entries: Vec<(InterpValue, InterpValue)>) -> Self {
        Self::from_entries(entries, None)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Operation {
    entry: Option<(InterpValue, InterpValue)>,
}

impl Operation {
    pub(crate) fn push(priority: InterpValue, value: InterpValue) -> Self {
        Self {
            entry: Some((priority, value)),
        }
    }
    pub(crate) fn pop() -> Self {
        Self { entry: None }
    }
}

#[derive(Clone, Copy, Debug)]
enum Stage {
    Rebuild(usize),
    Apply,
    Up(usize),
    Down(usize),
    Done,
}

#[derive(Clone, Copy, Debug)]
enum Comparison {
    Up {
        child: usize,
        parent: usize,
    },
    Children {
        root: usize,
        left: usize,
        right: usize,
    },
    Down {
        root: usize,
        child: usize,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct Update {
    queue: PriorityQueueValue,
    operation: Option<Operation>,
    was_pop: bool,
    popped: Option<InterpValue>,
    stage: Stage,
    rebuilding: Option<usize>,
    pending: Option<Comparison>,
}

pub(crate) enum UpdateStep {
    Continue,
    Compare,
    Complete,
}

pub(crate) struct ComparisonCall {
    pub target: Option<CallTarget>,
    pub left: InterpValue,
    pub right: InterpValue,
}

pub(crate) struct UpdateResult {
    pub queue: PriorityQueueValue,
    pub popped: Option<InterpValue>,
    pub was_pop: bool,
}

impl Update {
    fn compare(&mut self, comparison: Comparison) -> Result<UpdateStep, String> {
        #[cfg(test)]
        tests::record_comparison();
        self.pending = Some(comparison);
        Ok(UpdateStep::Compare)
    }

    pub(crate) fn comparison(&self) -> Result<ComparisonCall, String> {
        let (left, right) = match self
            .pending
            .ok_or("priority comparison has no pending request")?
        {
            Comparison::Up { child, parent } => (child, parent),
            Comparison::Children { left, right, .. } => (left, right),
            Comparison::Down { root, child } => (child, root),
        };
        let data = &self.queue.0;
        let priority = |index: usize| {
            data.heap
                .get(index)
                .and_then(|slot| data.slots.get(*slot))
                .and_then(Option::as_ref)
                .map(|pair| pair.0.clone())
                .ok_or_else(|| "priority heap references a vacant slot".to_owned())
        };
        let left = priority(left)?;
        let right = priority(right)?;
        Ok(ComparisonCall {
            target: data.comparator.clone(),
            left,
            right,
        })
    }

    pub(crate) fn finish(self) -> Result<UpdateResult, String> {
        if !matches!(self.stage, Stage::Done) || self.operation.is_some() || self.pending.is_some()
        {
            return Err("priority update is not complete".into());
        }
        Ok(UpdateResult {
            queue: self.queue,
            popped: self.popped,
            was_pop: self.was_pop,
        })
    }

    fn finish_sift(&mut self) {
        self.stage = match self.rebuilding.take() {
            Some(next) => Stage::Rebuild(next),
            None => Stage::Done,
        };
    }

    fn apply_comparison(&mut self, order: Ordering) -> Result<(), String> {
        let comparison = self
            .pending
            .take()
            .ok_or("priority comparison has no pending request")?;
        let data = Rc::make_mut(&mut self.queue.0);
        let above = |left: usize, right: usize| {
            order == Ordering::Greater
                || (order == Ordering::Equal && data.heap[left] < data.heap[right])
        };
        match comparison {
            Comparison::Up { child, parent } => {
                if above(child, parent) {
                    #[cfg(test)]
                    tests::record_swap();
                    data.heap.swap(child, parent);
                    self.stage = Stage::Up(parent);
                } else {
                    self.finish_sift();
                }
            }
            Comparison::Children { root, left, right } => {
                let child = if above(left, right) { left } else { right };
                // The child comparison is a separate machine step, not a Rust call.
                self.stage = Stage::Down(root);
                self.pending = Some(Comparison::Down { root, child });
            }
            Comparison::Down { root, child } => {
                if above(child, root) {
                    #[cfg(test)]
                    tests::record_swap();
                    data.heap.swap(root, child);
                    self.stage = Stage::Down(child);
                } else {
                    self.finish_sift();
                }
            }
        }
        Ok(())
    }

    pub(crate) fn advance(&mut self, order: Option<Ordering>) -> Result<UpdateStep, String> {
        if let Some(order) = order {
            self.apply_comparison(order)?;
            if let Some(comparison) = self.pending.take() {
                return self.compare(comparison);
            }
            return Ok(UpdateStep::Continue);
        }
        if self.pending.is_some() {
            return Err("priority comparison result is missing".into());
        }
        match self.stage {
            Stage::Rebuild(next) => {
                if next == 0 {
                    Rc::make_mut(&mut self.queue.0).ready = true;
                    self.stage = Stage::Apply;
                } else {
                    self.rebuilding = Some(next - 1);
                    self.stage = Stage::Down(next - 1);
                }
            }
            Stage::Apply => {
                let operation = self
                    .operation
                    .take()
                    .ok_or("priority update was already applied")?;
                let data = Rc::make_mut(&mut self.queue.0);
                match operation.entry {
                    Some((priority, value)) => {
                        data.heap.push(data.slots.len());
                        data.slots.push(Some((priority, value)));
                        self.stage = Stage::Up(data.heap.len() - 1);
                    }
                    None => {
                        if !data.heap.is_empty() {
                            let slot = data.heap.swap_remove(0);
                            self.popped = data.slots[slot].take().map(|(_, value)| value);
                        }
                        self.stage = Stage::Down(0);
                    }
                }
            }
            Stage::Up(child) => {
                if child == 0 {
                    self.finish_sift();
                } else {
                    return self.compare(Comparison::Up {
                        child,
                        parent: (child - 1) / 2,
                    });
                }
            }
            Stage::Down(root) => {
                let left = root * 2 + 1;
                let right = left + 1;
                let count = self.queue.len();
                if left >= count {
                    self.finish_sift();
                } else if right == count {
                    return self.compare(Comparison::Down { root, child: left });
                } else {
                    return self.compare(Comparison::Children { root, left, right });
                }
            }
            Stage::Done => {
                let data = Rc::make_mut(&mut self.queue.0);
                if data.heap.is_empty() {
                    data.slots.clear();
                } else if data.slots.len() > data.heap.len().saturating_mul(2) {
                    #[cfg(test)]
                    tests::record_compaction(data.slots.len());
                    let mut remap = vec![0; data.slots.len()];
                    let mut live = 0;
                    let mut old = 0;
                    data.slots.retain(|slot| {
                        let keep = slot.is_some();
                        if keep {
                            remap[old] = live;
                            live += 1;
                        }
                        old += 1;
                        keep
                    });
                    for slot in &mut data.heap {
                        *slot = remap[*slot];
                    }
                }
                return Ok(UpdateStep::Complete);
            }
        }
        Ok(UpdateStep::Continue)
    }
}

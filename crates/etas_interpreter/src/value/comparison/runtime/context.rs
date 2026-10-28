use std::cell::RefCell;

use super::memo::{States, Table};
use super::{Children, Comparison, Cursor};
use crate::value::identity::ValueIdentity;

#[derive(Clone)]
pub(super) enum Pair {
    Frames([usize; 4], [usize; 4]),
    Targets(usize, usize),
    TargetLists(usize, usize),
    Values(ValueIdentity, ValueIdentity),
}

const ACTIVE: u32 = 0;
const INVALID: u32 = u32::MAX;

struct Pairs {
    frames: Table<([usize; 4], [usize; 4])>,
    targets: Table<(usize, usize)>,
    lists: Table<(usize, usize)>,
    values: Table<(ValueIdentity, ValueIdentity)>,
    states: States,
    generation: u32,
}

impl Default for Pairs {
    fn default() -> Self {
        Self {
            frames: Table::default(),
            targets: Table::default(),
            lists: Table::default(),
            values: Table::default(),
            states: States::default(),
            generation: 1,
        }
    }
}

impl Pairs {
    fn get(&self, pair: &Pair) -> Option<usize> {
        match pair {
            Pair::Frames(a, b) => self.frames.get(&(*a, *b)),
            Pair::Targets(a, b) => self.targets.get(&(*a, *b)),
            Pair::TargetLists(a, b) => self.lists.get(&(*a, *b)),
            Pair::Values(a, b) => self.values.get(&(a.clone(), b.clone())),
        }
    }
    fn insert(&mut self, pair: Pair, slot: usize) {
        match pair {
            Pair::Frames(a, b) => self.frames.insert((a, b), slot),
            Pair::Targets(a, b) => self.targets.insert((a, b), slot),
            Pair::TargetLists(a, b) => self.lists.insert((a, b), slot),
            Pair::Values(a, b) => self.values.insert((a, b), slot),
        };
    }
}

#[derive(Default)]
pub(super) struct Context {
    pairs: RefCell<Pairs>,
    #[cfg(test)]
    pub(super) visits: std::cell::Cell<usize>,
}

impl Context {
    pub(super) fn known(&self, pair: &Pair) -> bool {
        let pairs = self.pairs.borrow();
        pairs.get(pair).is_some_and(|slot| {
            pairs.states.get(slot) == ACTIVE || pairs.states.get(slot) == pairs.generation
        })
    }

    pub(super) fn memoized_result<'c>(
        &'c self,
        pair: Option<Pair>,
        result: Comparison<Cursor<'c>>,
    ) -> Comparison<Cursor<'c>> {
        match (pair, result) {
            (Some(pair), Comparison::Pending(cursor)) if cursor.pair.is_none() => {
                self.memoized(pair, || Some(cursor.children))
            }
            (_, result) => result,
        }
    }
    pub(super) fn visit(&self) {
        #[cfg(test)]
        self.visits.set(self.visits.get() + 1);
    }
    pub(super) fn pending(&self, children: Children) -> Comparison<Cursor<'_>> {
        Comparison::Pending(Cursor {
            context: self,
            pair: None,
            children,
            index: 0,
        })
    }
    pub(super) fn memoized(
        &self,
        pair: Pair,
        children: impl FnOnce() -> Option<Children>,
    ) -> Comparison<Cursor<'_>> {
        let slot = {
            let pairs = self.pairs.borrow();
            let slot = pairs.get(&pair);
            if slot.is_some_and(|slot| {
                pairs.states.get(slot) == ACTIVE || pairs.states.get(slot) == pairs.generation
            }) {
                return Comparison::Ready(true);
            }
            slot
        };
        let Some(children) = children() else {
            return Comparison::Ready(false);
        };
        let mut pairs = self.pairs.borrow_mut();
        let slot = slot.unwrap_or_else(|| {
            let slot = pairs.states.push(INVALID);
            pairs.insert(pair, slot);
            slot
        });
        pairs.states.set(slot, ACTIVE);
        Comparison::Pending(Cursor {
            context: self,
            // An allocated Vec index is below isize::MAX; the one-based ticket
            // fits and keeps optional completion state to one machine word.
            pair: std::num::NonZeroUsize::new(slot + 1),
            children,
            index: 0,
        })
    }
    pub(super) fn finish(&self, slot: Option<std::num::NonZeroUsize>, equal: bool) {
        if equal && slot.is_none() {
            return;
        }
        let mut pairs = self.pairs.borrow_mut();
        if let Some(slot) = slot {
            let slot = slot.get() - 1;
            let state = if equal { pairs.generation } else { INVALID };
            pairs.states.set(slot, state);
        }
        if !equal {
            // Cyclic proofs can depend on active ancestors. Invalidate completed
            // proofs in O(1) when a candidate fails, retaining the active path.
            if pairs.generation < INVALID - 1 {
                pairs.generation += 1;
            } else {
                pairs.states.invalidate_completed(ACTIVE, INVALID);
                pairs.generation = 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::CallTargetChildren;

    #[test]
    fn generation_rollover_invalidates_completed_proofs_but_keeps_active_cycles() {
        let context = Context::default();
        let completed = Pair::Targets(1, 2);
        let active = Pair::Targets(3, 4);
        let empty = || {
            Some(Children::Targets(
                CallTargetChildren::from(Vec::new()),
                CallTargetChildren::from(Vec::new()),
            ))
        };
        assert!(super::super::compare(
            context.memoized(completed.clone(), empty)
        ));
        let active_cursor = context.memoized(active.clone(), empty);
        context.pairs.borrow_mut().generation = INVALID - 1;
        context.finish(None, false);
        assert_eq!(context.pairs.borrow().generation, 1);
        assert!(matches!(
            context.memoized(completed, || None),
            Comparison::Ready(false)
        ));
        assert!(matches!(
            context.memoized(active, || None),
            Comparison::Ready(true)
        ));
        assert!(super::super::compare(active_cursor));
    }
}

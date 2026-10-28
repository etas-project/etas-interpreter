use super::{Frame, InterpValue, Rc, SymbolId};

pub(crate) struct FrameComparison {
    left: Frame,
    right: Frame,
    slot: usize,
    restored_keys: std::vec::IntoIter<SymbolId>,
}

pub(crate) enum FrameComparisonStep<T> {
    Child(T),
    Complete(bool),
}

impl FrameComparison {
    pub(crate) fn identity(frame: &Frame) -> [usize; 4] {
        [
            Rc::as_ptr(&frame.slots) as usize,
            frame
                .restored
                .as_ref()
                .map_or(0, |v| Rc::as_ptr(v) as usize),
            std::sync::Arc::as_ptr(&frame.layout) as usize,
            std::sync::Arc::as_ptr(&frame.type_bindings) as usize,
        ]
    }

    pub(crate) fn new(left: &Frame, right: &Frame) -> Option<Self> {
        if left.layout != right.layout || left.type_bindings != right.type_bindings {
            return None;
        }
        let a = left.slots.try_borrow().ok()?;
        let b = right.slots.try_borrow().ok()?;
        if a.values.len() != b.values.len() || a.additional != b.additional {
            return None;
        }
        let restored_keys = match (&left.restored, &right.restored) {
            (None, None) => Vec::new(),
            (Some(a), Some(b)) => {
                let a = a.try_borrow().ok()?;
                let b = b.try_borrow().ok()?;
                if a.len() != b.len() {
                    return None;
                }
                // Retain only sparse symbol IDs, never copies of captured values.
                a.keys().copied().collect()
            }
            _ => return None,
        };
        Some(Self {
            left: left.clone(),
            right: right.clone(),
            slot: 0,
            restored_keys: restored_keys.into_iter(),
        })
    }

    pub(crate) fn next<T>(
        &mut self,
        compare: impl FnOnce(&InterpValue, &InterpValue) -> T,
    ) -> FrameComparisonStep<T> {
        let (Ok(a), Ok(b)) = (self.left.slots.try_borrow(), self.right.slots.try_borrow()) else {
            return FrameComparisonStep::Complete(false);
        };
        while self.slot < a.values.len() {
            let position = self.slot;
            self.slot += 1;
            match (&a.values[position], &b.values[position]) {
                (Some(a), Some(b)) => return FrameComparisonStep::Child(compare(a, b)),
                (None, None) => {}
                _ => return FrameComparisonStep::Complete(false),
            }
        }
        let Some(symbol) = self.restored_keys.next() else {
            return FrameComparisonStep::Complete(true);
        };
        let (Some(a), Some(b)) = (&self.left.restored, &self.right.restored) else {
            return FrameComparisonStep::Complete(false);
        };
        let (Ok(a), Ok(b)) = (a.try_borrow(), b.try_borrow()) else {
            return FrameComparisonStep::Complete(false);
        };
        match (a.get(&symbol), b.get(&symbol)) {
            (Some(a), Some(b)) => FrameComparisonStep::Child(compare(a, b)),
            _ => FrameComparisonStep::Complete(false),
        }
    }
}

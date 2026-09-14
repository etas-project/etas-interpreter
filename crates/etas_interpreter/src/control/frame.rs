use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

use etas_hir::SymbolId;

use crate::{plan::SlotLayoutTable, value::InterpValue};

#[derive(Clone, Debug)]
pub struct Frame {
    snapshot_id: u64,
    layout: Arc<SlotLayoutTable>,
    slots: Rc<RefCell<Vec<Option<InterpValue>>>>,
    restored: Option<Rc<RefCell<HashMap<SymbolId, InterpValue>>>>,
    type_bindings: Arc<HashMap<String, etas_types::TypeId>>,
}

static NEXT_FRAME_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        self.layout == other.layout
            && self.slots == other.slots
            && self.restored == other.restored
            && self.type_bindings == other.type_bindings
    }
}
impl Eq for Frame {}

impl Frame {
    pub fn new(layout: Arc<SlotLayoutTable>) -> Self {
        Self::with_shared_type_bindings(layout, Arc::new(HashMap::new()))
    }

    fn with_shared_type_bindings(
        layout: Arc<SlotLayoutTable>,
        type_bindings: Arc<HashMap<String, etas_types::TypeId>>,
    ) -> Self {
        let slot_count = layout.slot_count();
        Self {
            snapshot_id: NEXT_FRAME_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            layout,
            slots: Rc::new(RefCell::new(vec![None; slot_count])),
            restored: None,
            type_bindings,
        }
    }

    pub fn with_type_bindings(
        layout: Arc<SlotLayoutTable>,
        type_bindings: HashMap<String, etas_types::TypeId>,
    ) -> Self {
        Self::with_shared_type_bindings(layout, Arc::new(type_bindings))
    }

    pub(crate) fn capture(
        &self,
        layout: Arc<SlotLayoutTable>,
        symbols: &[SymbolId],
    ) -> Result<Self, String> {
        let mut captured = Self::with_shared_type_bindings(layout, self.type_bindings.clone());
        for symbol in symbols {
            if captured.layout.resolve(*symbol).is_none() {
                return Err(format!(
                    "closure layout is missing captured symbol {symbol:?}"
                ));
            }
            let value = self
                .get(*symbol)
                .ok_or_else(|| format!("closure capture {symbol:?} is not bound"))?;
            captured.insert(*symbol, value);
        }
        Ok(captured)
    }

    pub(crate) fn from_snapshot(locals: Vec<(SymbolId, InterpValue)>) -> Result<Self, String> {
        let mut restored = HashMap::with_capacity(locals.len());
        for (symbol, value) in locals {
            if restored.insert(symbol, value).is_some() {
                return Err(format!(
                    "snapshot frame contains duplicate local symbol {}",
                    symbol.0
                ));
            }
        }
        Ok(Self {
            snapshot_id: NEXT_FRAME_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            layout: Arc::new(SlotLayoutTable::default()),
            slots: Rc::new(RefCell::new(Vec::new())),
            restored: Some(Rc::new(RefCell::new(restored))),
            type_bindings: Arc::new(HashMap::new()),
        })
    }

    pub(crate) fn from_snapshot_with_type_bindings(
        locals: Vec<(SymbolId, InterpValue)>,
        type_bindings: HashMap<String, etas_types::TypeId>,
    ) -> Result<Self, String> {
        let mut frame = Self::from_snapshot(locals)?;
        frame.type_bindings = Arc::new(type_bindings);
        Ok(frame)
    }

    pub fn type_bindings(&self) -> &HashMap<String, etas_types::TypeId> {
        &self.type_bindings
    }

    pub(crate) fn snapshot_id(&self) -> u64 {
        self.snapshot_id
    }

    pub(crate) fn set_snapshot_id(&mut self, id: u64) -> Result<(), String> {
        if id == 0 {
            return Err("snapshot frame identity must be nonzero".into());
        }
        self.snapshot_id = id;
        Ok(())
    }

    pub fn sorted_type_bindings(&self) -> Vec<(String, etas_types::TypeId)> {
        let mut bindings = self
            .type_bindings
            .iter()
            .map(|(name, ty)| (name.clone(), *ty))
            .collect::<Vec<_>>();
        bindings.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0));
        bindings
    }

    pub fn get(&self, symbol: SymbolId) -> Option<InterpValue> {
        if let Some(slot) = self.layout.resolve(symbol) {
            return self.slots.borrow()[slot.0 as usize].clone();
        }
        self.restored
            .as_ref()
            .and_then(|locals| locals.borrow().get(&symbol).cloned())
    }

    pub fn insert(&mut self, symbol: SymbolId, value: InterpValue) {
        if let Some(slot) = self.layout.resolve(symbol) {
            self.slots.borrow_mut()[slot.0 as usize] = Some(value);
            return;
        }
        if let Some(locals) = &self.restored {
            locals.borrow_mut().insert(symbol, value);
            return;
        }
        panic!("missing slot layout for symbol {:?}", symbol);
    }

    pub fn set(&mut self, symbol: SymbolId, value: InterpValue) -> bool {
        if let Some(slot) = self.layout.resolve(symbol) {
            let mut slots = self.slots.borrow_mut();
            let index = slot.0 as usize;
            if slots[index].is_some() {
                slots[index] = Some(value);
                return true;
            }
            return false;
        }
        self.restored.as_ref().is_some_and(|locals| {
            let mut locals = locals.borrow_mut();
            if let std::collections::hash_map::Entry::Occupied(mut entry) = locals.entry(symbol) {
                entry.insert(value);
                true
            } else {
                false
            }
        })
    }

    /// Synchronous commit only: no evaluation or suspension while the slot is borrowed.
    pub(crate) fn with_local_mut<R>(
        &mut self,
        symbol: SymbolId,
        update: impl FnOnce(&mut InterpValue) -> R,
    ) -> Option<R> {
        if let Some(slot) = self.layout.resolve(symbol) {
            let mut slots = self.slots.borrow_mut();
            return slots.get_mut(slot.0 as usize)?.as_mut().map(update);
        }
        let mut locals = self.restored.as_ref()?.borrow_mut();
        locals.get_mut(&symbol).map(update)
    }

    pub fn snapshot_symbols(&self) -> HashSet<SymbolId> {
        let mut symbols = self
            .layout
            .symbols()
            .iter()
            .enumerate()
            .filter_map(|(index, symbol)| self.slots.borrow()[index].as_ref().map(|_| *symbol))
            .collect::<HashSet<_>>();
        if let Some(restored) = &self.restored {
            symbols.extend(restored.borrow().keys().copied());
        }
        symbols
    }

    pub fn cleanup_to(&mut self, keep: &HashSet<SymbolId>) {
        let mut slots = self.slots.borrow_mut();
        for (index, symbol) in self.layout.symbols().iter().enumerate() {
            if !keep.contains(symbol) {
                slots[index] = None;
            }
        }
        if let Some(restored) = &self.restored {
            restored
                .borrow_mut()
                .retain(|symbol, _| keep.contains(symbol));
        }
    }

    pub fn sorted_locals(&self) -> Vec<(SymbolId, InterpValue)> {
        let slots = self.slots.borrow();
        let mut locals = self
            .layout
            .symbols()
            .iter()
            .enumerate()
            .filter_map(|(index, symbol)| slots[index].clone().map(|value| (*symbol, value)))
            .collect::<Vec<_>>();
        if let Some(restored) = &self.restored {
            locals.extend(
                restored
                    .borrow()
                    .iter()
                    .map(|(symbol, value)| (*symbol, value.clone())),
            );
        }
        locals.sort_by_key(|(symbol, _)| symbol.0);
        locals
    }

    pub(crate) fn try_map_locals<T, E>(
        &self,
        mut map: impl FnMut(&InterpValue) -> Result<T, E>,
    ) -> Result<Vec<(SymbolId, T)>, E> {
        let slots = self.slots.borrow();
        let restored = self.restored.as_ref().map(|locals| locals.borrow());
        let count = slots.iter().filter(|value| value.is_some()).count()
            + restored.as_ref().map_or(0, |locals| locals.len());
        let mut output = Vec::with_capacity(count);
        for (symbol, value) in self.layout.symbols().iter().zip(slots.iter()) {
            if let Some(value) = value {
                output.push((*symbol, map(value)?));
            }
        }
        if let Some(restored) = restored {
            for (symbol, value) in restored.iter() {
                output.push((*symbol, map(value)?));
            }
        }
        output.sort_unstable_by_key(|(symbol, _)| symbol.0);
        Ok(output)
    }
}

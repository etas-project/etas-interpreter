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
    slots: Rc<RefCell<SlotStorage>>,
    restored: Option<Rc<RefCell<HashMap<SymbolId, InterpValue>>>>,
    type_bindings: Arc<HashMap<String, etas_types::TypeId>>,
}

#[derive(Debug, PartialEq, Eq)]
struct SlotStorage {
    values: Vec<Option<InterpValue>>,
    additional: HashMap<SymbolId, usize>,
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
            slots: Rc::new(RefCell::new(SlotStorage {
                values: vec![None; slot_count],
                additional: HashMap::new(),
            })),
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
            slots: Rc::new(RefCell::new(SlotStorage {
                values: Vec::new(),
                additional: HashMap::new(),
            })),
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

    // Frame aliases share slots, restored locals, layout and immutable type bindings.
    // Include the wire identity separately: decoding may assign a different ID.
    pub(crate) fn shared_capture_identity(&self) -> Option<(*const (), u64)> {
        (Rc::strong_count(&self.slots) > 1)
            .then(|| (Rc::as_ptr(&self.slots).cast(), self.snapshot_id))
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
        let slots = self.slots.borrow();
        if let Some(slot) = self.resolve(&slots, symbol) {
            return slots.values[slot].clone();
        }
        self.restored
            .as_ref()
            .and_then(|locals| locals.borrow().get(&symbol).cloned())
    }

    pub fn insert(&mut self, symbol: SymbolId, value: InterpValue) {
        let mut slots = self.slots.borrow_mut();
        if let Some(slot) = self.resolve(&slots, symbol) {
            slots.values[slot] = Some(value);
            return;
        }
        if let Some(locals) = &self.restored {
            locals.borrow_mut().insert(symbol, value);
            return;
        }
        panic!("missing slot layout for symbol {:?}", symbol);
    }

    pub fn set(&mut self, symbol: SymbolId, value: InterpValue) -> bool {
        let mut slots = self.slots.borrow_mut();
        if let Some(index) = self.resolve(&slots, symbol) {
            if slots.values[index].is_some() {
                slots.values[index] = Some(value);
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
        let mut slots = self.slots.borrow_mut();
        if let Some(slot) = self.resolve(&slots, symbol) {
            return slots.values.get_mut(slot)?.as_mut().map(update);
        }
        let mut locals = self.restored.as_ref()?.borrow_mut();
        locals.get_mut(&symbol).map(update)
    }

    pub fn snapshot_symbols(&self) -> HashSet<SymbolId> {
        let slots = self.slots.borrow();
        let mut symbols = self
            .slot_symbols(&slots)
            .filter_map(|(symbol, index)| slots.values[index].as_ref().map(|_| symbol))
            .collect::<HashSet<_>>();
        if let Some(restored) = &self.restored {
            symbols.extend(restored.borrow().keys().copied());
        }
        symbols
    }

    pub fn cleanup_to(&mut self, keep: &HashSet<SymbolId>) {
        let mut slots = self.slots.borrow_mut();
        let SlotStorage { values, additional } = &mut *slots;
        for (symbol, index) in self
            .layout
            .symbols()
            .iter()
            .copied()
            .enumerate()
            .map(|(index, symbol)| (symbol, index))
            .chain(additional.iter().map(|(symbol, index)| (*symbol, *index)))
        {
            if !keep.contains(&symbol) {
                values[index] = None;
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
            .slot_symbols(&slots)
            .filter_map(|(symbol, index)| slots.values[index].clone().map(|value| (symbol, value)))
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
        let count = slots.values.iter().filter(|value| value.is_some()).count()
            + restored.as_ref().map_or(0, |locals| locals.len());
        let mut output = Vec::with_capacity(count);
        for (symbol, index) in self.slot_symbols(&slots) {
            if let Some(value) = &slots.values[index] {
                output.push((symbol, map(value)?));
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

    fn resolve(&self, slots: &SlotStorage, symbol: SymbolId) -> Option<usize> {
        self.layout
            .resolve(symbol)
            .map(|slot| slot.0 as usize)
            .or_else(|| slots.additional.get(&symbol).copied())
    }

    fn slot_symbols<'a>(
        &'a self,
        slots: &'a SlotStorage,
    ) -> impl Iterator<Item = (SymbolId, usize)> + 'a {
        self.layout
            .symbols()
            .iter()
            .copied()
            .enumerate()
            .map(|(index, symbol)| (symbol, index))
            .chain(
                slots
                    .additional
                    .iter()
                    .map(|(symbol, index)| (*symbol, *index)),
            )
    }

    /// A first-class handler may be defined outside the applying callable.
    /// Install only its planned bindings, in shared storage so every alias keeps
    /// the same frame identity and observes the same symbol-to-slot mapping.
    pub(crate) fn install_scope_layout(&mut self, layout: &SlotLayoutTable) {
        // Restored frames already use symbolic locals rather than dense slots.
        if self.restored.is_some() {
            return;
        }
        let mut slots = self.slots.borrow_mut();
        let missing = layout
            .symbols()
            .iter()
            .filter(|symbol| self.resolve(&slots, **symbol).is_none())
            .count();
        slots.values.reserve(missing);
        slots.additional.reserve(missing);
        for symbol in layout.symbols() {
            if self.resolve(&slots, *symbol).is_none() {
                let index = slots.values.len();
                slots.additional.insert(*symbol, index);
                slots.values.push(None);
            }
        }
    }
}

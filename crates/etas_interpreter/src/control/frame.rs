use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

use etas_hir::SymbolId;

use crate::{plan::SlotLayoutTable, value::InterpValue};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    layout: Arc<SlotLayoutTable>,
    slots: Rc<RefCell<Vec<Option<InterpValue>>>>,
    restored: Option<Rc<RefCell<HashMap<SymbolId, InterpValue>>>>,
    type_bindings: Arc<HashMap<String, etas_types::TypeId>>,
}

impl Frame {
    pub fn new(layout: Arc<SlotLayoutTable>) -> Self {
        let slot_count = layout.slot_count();
        Self {
            layout,
            slots: Rc::new(RefCell::new(vec![None; slot_count])),
            restored: None,
            type_bindings: Arc::new(HashMap::new()),
        }
    }

    pub fn with_type_bindings(
        layout: Arc<SlotLayoutTable>,
        type_bindings: HashMap<String, etas_types::TypeId>,
    ) -> Self {
        let mut frame = Self::new(layout);
        frame.type_bindings = Arc::new(type_bindings);
        frame
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
}

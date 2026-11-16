use std::{
    cell::{Ref, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

use etas_hir::SymbolId;

use crate::{plan::SlotLayoutTable, value::InterpValue};

mod comparison;
pub(crate) use comparison::{FrameComparison, FrameComparisonStep};

#[derive(Clone, Debug)]
pub struct Frame {
    snapshot_id: u64,
    parent: Option<FrameParent>,
    residual_invocation: Option<std::num::NonZeroU64>,
    layout: Arc<SlotLayoutTable>,
    slots: Rc<RefCell<SlotStorage>>,
    restored: Option<Rc<RefCell<RestoredStorage>>>,
    type_bindings: Arc<super::GenericBindings>,
}

/// Shared lexical bindings; request-local slots remain in the child frame.
#[derive(Clone, Debug)]
struct FrameParent(Rc<Frame>);
impl Drop for FrameParent {
    fn drop(&mut self) {
        let mut next = Rc::get_mut(&mut self.0).and_then(|frame| frame.parent.take());
        while let Some(mut parent) = next {
            next = Rc::get_mut(&mut parent.0).and_then(|frame| frame.parent.take());
        }
    }
}

pub(crate) struct FrameLocals<'a> {
    frame: &'a Frame,
    slots: Ref<'a, SlotStorage>,
    restored: Option<Ref<'a, RestoredStorage>>,
}

impl FrameLocals<'_> {
    pub(crate) fn sorted(&self) -> Vec<(SymbolId, &InterpValue)> {
        let mut locals = self
            .frame
            .slot_symbols(&self.slots)
            .filter_map(|(symbol, index)| {
                self.slots.values[index]
                    .as_ref()
                    .map(|value| (symbol, value))
            })
            .collect::<Vec<_>>();
        if let Some(restored) = &self.restored {
            locals.extend(restored.iter().map(|(symbol, value)| (*symbol, value)));
        }
        locals.sort_unstable_by_key(|(symbol, _)| symbol.0);
        locals
    }
}

#[derive(Debug)]
struct SlotStorage {
    handler_activations: std::collections::BTreeMap<
        etas_hir::HirHandlerArmId,
        super::handler_activation::HandlerActivation,
    >,
    values: Vec<Option<InterpValue>>,
    additional: HashMap<SymbolId, usize>,
    scope_symbols: Option<Rc<HashSet<SymbolId>>>,
}

#[derive(Debug)]
struct RestoredStorage(HashMap<SymbolId, InterpValue>);

impl std::ops::Deref for RestoredStorage {
    type Target = HashMap<SymbolId, InterpValue>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for RestoredStorage {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for RestoredStorage {
    fn drop(&mut self) {
        for value in std::mem::take(&mut self.0).into_values() {
            crate::value::release_value(value);
        }
    }
}
impl Drop for SlotStorage {
    fn drop(&mut self) {
        for value in std::mem::take(&mut self.values).into_iter().flatten() {
            crate::value::release_value(value);
        }
    }
}

pub(crate) struct FrameValues {
    _parent: Option<FrameParent>,
    slots: std::vec::IntoIter<Option<InterpValue>>,
    restored: Option<std::collections::hash_map::IntoValues<SymbolId, InterpValue>>,
}
impl FrameValues {
    pub(crate) fn is_empty(&self) -> bool {
        self.slots.len() == 0
            && self
                .restored
                .as_ref()
                .is_none_or(|values| values.len() == 0)
    }
}
impl Iterator for FrameValues {
    type Item = InterpValue;
    fn next(&mut self) -> Option<Self::Item> {
        self.slots
            .find_map(|value| value)
            .or_else(|| self.restored.as_mut()?.next())
    }
}

static NEXT_FRAME_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        crate::value::comparison::frames_equal(self, other)
    }
}
impl Eq for Frame {}

impl Frame {
    pub(crate) fn borrow_locals(&self) -> Result<FrameLocals<'_>, String> {
        let conflict = || "runtime frame is mutably borrowed during JSON projection".to_owned();
        Ok(FrameLocals {
            frame: self,
            slots: self.slots.try_borrow().map_err(|_| conflict())?,
            restored: self
                .restored
                .as_ref()
                .map(|locals| locals.try_borrow().map_err(|_| conflict()))
                .transpose()?,
        })
    }

    pub fn new(layout: Arc<SlotLayoutTable>) -> Self {
        Self::with_shared_type_bindings(layout, super::GenericBindings::new().shared())
    }

    fn with_shared_type_bindings(
        layout: Arc<SlotLayoutTable>,
        type_bindings: Arc<super::GenericBindings>,
    ) -> Self {
        let slot_count = layout.slot_count();
        Self {
            snapshot_id: NEXT_FRAME_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            residual_invocation: None,
            parent: None,
            layout,
            slots: Rc::new(RefCell::new(SlotStorage {
                values: vec![None; slot_count],
                additional: HashMap::new(),
                scope_symbols: None,
                handler_activations: Default::default(),
            })),
            restored: None,
            type_bindings,
        }
    }

    pub fn with_type_bindings(
        layout: Arc<SlotLayoutTable>,
        type_bindings: HashMap<String, etas_types::TypeId>,
    ) -> Self {
        Self::with_shared_type_bindings(
            layout,
            type_bindings
                .into_iter()
                .collect::<super::GenericBindings>()
                .shared(),
        )
    }

    pub(crate) fn with_generic_bindings(
        layout: Arc<SlotLayoutTable>,
        bindings: super::GenericBindings,
    ) -> Self {
        Self::with_shared_type_bindings(layout, bindings.shared())
    }

    pub(crate) fn for_handler(&self, layout: Arc<SlotLayoutTable>) -> Self {
        let mut frame = Self::with_shared_type_bindings(layout, self.type_bindings.clone());
        frame.residual_invocation = self.residual_invocation;
        frame.parent = Some(FrameParent(Rc::new(self.clone())));
        frame
    }

    pub(crate) fn lexical_parent(&self) -> Option<&Frame> {
        self.parent.as_ref().map(|parent| parent.0.as_ref())
    }

    pub(crate) fn restore_lexical_parent(&mut self, parent: Frame) {
        self.parent = Some(FrameParent(Rc::new(parent)));
    }

    pub(crate) fn has_handler_activations(&self) -> bool {
        !self.slots.borrow().handler_activations.is_empty()
            || self
                .lexical_parent()
                .is_some_and(Self::has_handler_activations)
    }

    pub(crate) fn handler_activations(&self) -> Vec<super::handler_activation::HandlerActivation> {
        self.slots
            .borrow()
            .handler_activations
            .values()
            .cloned()
            .collect()
    }

    pub(crate) fn bind_handler_activation(
        &mut self,
        activation: super::handler_activation::HandlerActivation,
    ) {
        self.slots
            .borrow_mut()
            .handler_activations
            .insert(activation.arm, activation);
    }

    pub(crate) fn bindings_at<'a>(
        &'a self,
        checked: &etas_frontend::CheckedProgram,
        owner: etas_hir::HirOwner,
    ) -> std::borrow::Cow<'a, super::GenericBindings> {
        if super::handler_activation::parametric_continuation_call(checked, owner) {
            return std::borrow::Cow::Borrowed(&self.type_bindings);
        }
        let arm = super::handler_activation::enclosing_handler(checked, owner);
        let mut current = Some(self);
        while let Some(frame) = current {
            let slots = frame.slots.borrow();
            if let Some(activation) = arm.and_then(|arm| slots.handler_activations.get(&arm)) {
                return std::borrow::Cow::Owned(
                    super::handler_activation::activation_bindings(
                        &self.type_bindings,
                        Some(activation),
                    )
                    .into_owned(),
                );
            }
            current = frame.lexical_parent();
        }
        std::borrow::Cow::Borrowed(&self.type_bindings)
    }

    pub(crate) fn generic_bindings(&self) -> &super::GenericBindings {
        &self.type_bindings
    }

    pub(crate) fn capture(
        &self,
        layout: Arc<SlotLayoutTable>,
        symbols: &[SymbolId],
    ) -> Result<Self, String> {
        let mut captured = Self::with_shared_type_bindings(layout, self.type_bindings.clone());
        let mut current = Some(self);
        while let Some(frame) = current {
            for (arm, activation) in &frame.slots.borrow().handler_activations {
                captured
                    .slots
                    .borrow_mut()
                    .handler_activations
                    .entry(*arm)
                    .or_insert_with(|| activation.clone());
            }
            current = frame.lexical_parent();
        }
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
            residual_invocation: None,
            parent: None,
            layout: Arc::new(SlotLayoutTable::default()),
            slots: Rc::new(RefCell::new(SlotStorage {
                values: Vec::new(),
                additional: HashMap::new(),
                scope_symbols: None,
                handler_activations: Default::default(),
            })),
            restored: Some(Rc::new(RefCell::new(RestoredStorage(restored)))),
            type_bindings: super::GenericBindings::new().shared(),
        })
    }

    pub(crate) fn from_snapshot_with_generic_bindings(
        locals: Vec<(SymbolId, InterpValue)>,
        bindings: super::GenericBindings,
    ) -> Result<Self, String> {
        let mut frame = Self::from_snapshot(locals)?;
        frame.type_bindings = bindings.shared();
        Ok(frame)
    }

    pub fn type_bindings(&self) -> &HashMap<String, etas_types::TypeId> {
        &self.type_bindings
    }

    pub(crate) fn snapshot_id(&self) -> u64 {
        self.snapshot_id
    }

    pub(crate) fn residual_invocation(&self) -> Option<std::num::NonZeroU64> {
        self.residual_invocation
    }

    pub(crate) fn bind_residual_invocation(
        &mut self,
        invocation: std::num::NonZeroU64,
    ) -> Result<(), String> {
        if self.residual_invocation.is_some() {
            return Err("callable frame already owns a residual scope invocation".into());
        }
        self.residual_invocation = Some(invocation);
        Ok(())
    }

    // Frame aliases share slots, restored locals, layout and immutable type bindings.
    // Include the wire identity separately: decoding may assign a different ID.
    pub(crate) fn capture_identity(&self) -> (*const (), u64) {
        (Rc::as_ptr(&self.slots).cast(), self.snapshot_id)
    }

    pub(crate) fn into_unique_values(self) -> FrameValues {
        let slots = Rc::try_unwrap(self.slots)
            .ok()
            .map(|slots| std::mem::take(&mut slots.into_inner().values))
            .unwrap_or_default();
        let restored = self
            .restored
            .and_then(|locals| Rc::try_unwrap(locals).ok())
            .map(|locals| std::mem::take(&mut locals.into_inner().0).into_values());
        FrameValues {
            _parent: self.parent,
            slots: slots.into_iter(),
            restored,
        }
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
        let mut current = Some(self);
        while let Some(frame) = current {
            let slots = frame.slots.borrow();
            if let Some(slot) = frame.resolve(&slots, symbol) {
                return slots.values[slot].clone();
            }
            if let Some(value) = frame
                .restored
                .as_ref()
                .and_then(|locals| locals.borrow().get(&symbol).cloned())
            {
                return Some(value);
            }
            current = frame.lexical_parent();
        }
        None
    }

    pub fn insert(&mut self, symbol: SymbolId, value: InterpValue) {
        let mut slots = self.slots.borrow_mut();
        if let Some(slot) = self.resolve(&slots, symbol) {
            if slots.values[slot].is_none() {
                slots.scope_symbols = None;
            }
            slots.values[slot] = Some(value);
            return;
        }
        if let Some(locals) = &self.restored {
            if locals.borrow_mut().insert(symbol, value).is_none() {
                slots.scope_symbols = None;
            }
            return;
        }
        panic!("missing slot layout for symbol {:?}", symbol);
    }

    pub fn set(&mut self, symbol: SymbolId, value: InterpValue) -> bool {
        let mut current = Some(&*self);
        while let Some(frame) = current {
            let mut slots = frame.slots.borrow_mut();
            if let Some(index) = frame.resolve(&slots, symbol) {
                if slots.values[index].is_some() {
                    slots.values[index] = Some(value);
                    return true;
                }
                return false;
            }
            if let Some(locals) = &frame.restored {
                let mut locals = locals.borrow_mut();
                if let std::collections::hash_map::Entry::Occupied(mut entry) = locals.entry(symbol)
                {
                    entry.insert(value);
                    return true;
                }
            }
            current = frame.lexical_parent();
        }
        false
    }

    /// Synchronous commit only: no evaluation or suspension while the slot is borrowed.
    pub(crate) fn with_local_mut<R>(
        &mut self,
        symbol: SymbolId,
        update: impl FnOnce(&mut InterpValue) -> R,
    ) -> Option<R> {
        let mut current = Some(&*self);
        while let Some(frame) = current {
            let mut slots = frame.slots.borrow_mut();
            if let Some(slot) = frame.resolve(&slots, symbol) {
                return slots.values.get_mut(slot)?.as_mut().map(update);
            }
            if let Some(locals) = &frame.restored {
                let mut locals = locals.borrow_mut();
                if let Some(value) = locals.get_mut(&symbol) {
                    return Some(update(value));
                }
            }
            current = frame.lexical_parent();
        }
        None
    }

    pub fn snapshot_symbols(&self) -> HashSet<SymbolId> {
        let slots = self.slots.borrow();
        self.collect_symbols(&slots)
    }

    fn collect_symbols(&self, slots: &SlotStorage) -> HashSet<SymbolId> {
        let mut symbols = self
            .slot_symbols(slots)
            .filter_map(|(symbol, index)| slots.values[index].as_ref().map(|_| symbol))
            .collect::<HashSet<_>>();
        if let Some(restored) = &self.restored {
            symbols.extend(restored.borrow().keys().copied());
        }
        symbols
    }

    /// Reassignments preserve scope membership. Frame aliases share invalidation,
    /// while previously returned scope snapshots remain immutable.
    pub(crate) fn scope_symbols(&self) -> Rc<HashSet<SymbolId>> {
        let mut slots = self.slots.borrow_mut();
        if let Some(cached) = &slots.scope_symbols {
            return cached.clone();
        }
        let symbols = Rc::new(self.collect_symbols(&slots));
        slots.scope_symbols = Some(symbols.clone());
        symbols
    }

    pub fn cleanup_to(&mut self, keep: &HashSet<SymbolId>) {
        let mut slots = self.slots.borrow_mut();
        if slots
            .scope_symbols
            .as_ref()
            .is_some_and(|cached| std::ptr::eq(cached.as_ref(), keep))
        {
            return;
        }
        let SlotStorage {
            values, additional, ..
        } = &mut *slots;
        let mut removed = false;
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
                removed |= values[index].take().is_some();
            }
        }
        if let Some(restored) = &self.restored {
            let mut restored = restored.borrow_mut();
            let count = restored.len();
            restored.retain(|symbol, _| keep.contains(symbol));
            removed |= restored.len() != count;
        }
        if removed {
            slots.scope_symbols = None;
        }
    }

    pub fn sorted_locals(&self) -> Vec<(SymbolId, InterpValue)> {
        let mut locals = std::collections::BTreeMap::new();
        let mut current = Some(self);
        while let Some(frame) = current {
            let slots = frame.slots.borrow();
            for (symbol, index) in frame.slot_symbols(&slots) {
                if let Some(value) = &slots.values[index] {
                    locals.entry(symbol).or_insert_with(|| value.clone());
                }
            }
            if let Some(restored) = &frame.restored {
                for (symbol, value) in restored.borrow().iter() {
                    locals.entry(*symbol).or_insert_with(|| value.clone());
                }
            }
            current = frame.lexical_parent();
        }
        locals.into_iter().collect()
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

    pub(crate) fn try_visit_locals<E>(
        &self,
        mut visit: impl FnMut(&InterpValue) -> Result<(), E>,
    ) -> Result<(), E> {
        let slots = self.slots.borrow();
        for (_, index) in self.slot_symbols(&slots) {
            if let Some(value) = &slots.values[index] {
                visit(value)?;
            }
        }
        if let Some(restored) = &self.restored {
            for value in restored.borrow().values() {
                visit(value)?;
            }
        }
        Ok(())
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
    #[cfg(test)]
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

#[cfg(test)]
mod tests;

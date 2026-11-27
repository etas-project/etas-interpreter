use std::{
    cell::{Ref, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

use etas_hir::SymbolId;

use crate::{plan::SlotLayoutTable, value::InterpValue};

mod binding;
mod comparison;
pub(crate) use binding::{BindingIdentity, SharedBinding};
pub(crate) use comparison::{FrameComparison, FrameComparisonStep};

#[derive(Clone, Debug)]
pub struct Frame {
    snapshot_id: u64,
    parent: Option<FrameParent>,
    residual_invocation: Option<std::num::NonZeroU64>,
    storage: Rc<FrameStorage>,
    type_bindings: Arc<super::GenericBindings>,
    restored: Option<Rc<RefCell<RestoredStorage>>>,
}

#[derive(Debug)]
struct FrameStorage {
    layout: Arc<SlotLayoutTable>,
    slots: RefCell<SlotStorage>,
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
    shared: Vec<(SymbolId, InterpValue)>,
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
        locals.extend(self.shared.iter().map(|(symbol, value)| (*symbol, value)));
        locals.sort_unstable_by_key(|(symbol, _)| symbol.0);
        locals
    }
}

#[derive(Debug)]
struct SlotStorage {
    owner: u64,
    shared: HashMap<SymbolId, SharedBinding>,
    scoped_captures: bool,
    scoped_values: HashSet<SymbolId>,
    exclusive_receivers: HashSet<SymbolId>,
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
        for binding in self.shared.values() {
            if binding.owner == self.owner {
                binding.expire();
            }
        }
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
        let slots = self.storage.slots.try_borrow().map_err(|_| conflict())?;
        if !slots.exclusive_receivers.is_empty() {
            return Err("cannot capture a frame while an updating receiver is held".into());
        }
        let shared = slots
            .shared
            .iter()
            .map(|(symbol, binding)| {
                binding
                    .value()
                    .map(|value| (*symbol, value))
                    .ok_or_else(|| {
                        "captured binding is held or outside its owning scope".to_owned()
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(FrameLocals {
            frame: self,
            shared,
            slots,
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
        let snapshot_id = NEXT_FRAME_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            snapshot_id,
            residual_invocation: None,
            parent: None,
            type_bindings,
            storage: Rc::new(FrameStorage {
                layout,
                slots: RefCell::new(SlotStorage {
                    owner: snapshot_id,
                    shared: HashMap::new(),
                    scoped_captures: false,
                    scoped_values: HashSet::new(),
                    values: vec![None; slot_count],
                    additional: HashMap::new(),
                    scope_symbols: None,
                    handler_activations: Default::default(),
                    exclusive_receivers: HashSet::new(),
                }),
            }),
            restored: None,
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
        !self.storage.slots.borrow().handler_activations.is_empty()
            || self
                .lexical_parent()
                .is_some_and(Self::has_handler_activations)
    }

    pub(crate) fn handler_activations(&self) -> Vec<super::handler_activation::HandlerActivation> {
        self.storage
            .slots
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
        self.storage
            .slots
            .borrow_mut()
            .handler_activations
            .insert(activation.arm, activation);
    }

    pub(crate) fn bindings_at<'a>(
        &'a self,
        checked: &etas_frontend::CheckedProgram,
        owner: etas_hir::HirOwner,
    ) -> std::borrow::Cow<'a, super::GenericBindings> {
        if !self.has_handler_activations()
            || super::handler_activation::parametric_continuation_call(checked, owner)
        {
            return std::borrow::Cow::Borrowed(&self.type_bindings);
        }
        let arm = super::handler_activation::enclosing_handler(checked, owner);
        let mut current = Some(self);
        while let Some(frame) = current {
            let slots = frame.storage.slots.borrow();
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
            for (arm, activation) in &frame.storage.slots.borrow().handler_activations {
                captured
                    .storage
                    .slots
                    .borrow_mut()
                    .handler_activations
                    .entry(*arm)
                    .or_insert_with(|| activation.clone());
            }
            current = frame.lexical_parent();
        }
        for symbol in symbols {
            if captured.storage.layout.resolve(*symbol).is_none() {
                return Err(format!(
                    "closure layout is missing captured symbol {symbol:?}"
                ));
            }
            if let Some(binding) = self.shared_binding(*symbol) {
                captured.install_shared_binding(*symbol, binding);
                continue;
            }
            let value = self
                .get(*symbol)
                .ok_or_else(|| format!("closure capture {symbol:?} is not bound"))?;
            if crate::value::graph::has_scoped_bindings(crate::value::graph::Node::Value(&value)) {
                captured.mark_scoped_value(*symbol);
            }
            captured.insert(*symbol, value);
        }
        Ok(captured)
    }

    pub(crate) fn capture_environment(
        &self,
        layout: Arc<SlotLayoutTable>,
        symbols: &[SymbolId],
        mutable: &[SymbolId],
    ) -> Result<Self, String> {
        for symbol in mutable {
            self.share_binding(*symbol)?;
        }
        self.capture(layout, symbols)
    }

    fn shared_binding(&self, symbol: SymbolId) -> Option<SharedBinding> {
        let mut current = Some(self);
        while let Some(frame) = current {
            if let Some(binding) = frame.storage.slots.borrow().shared.get(&symbol) {
                return Some(binding.clone());
            }
            current = frame.lexical_parent();
        }
        None
    }

    fn share_binding(&self, symbol: SymbolId) -> Result<(), String> {
        let mut current = Some(self);
        while let Some(frame) = current {
            let mut slots = frame.storage.slots.borrow_mut();
            if slots.shared.contains_key(&symbol) {
                return Ok(());
            }
            if slots.exclusive_receivers.contains(&symbol) {
                return Err("cannot capture an exclusively held updating receiver".into());
            }
            let value = frame
                .resolve(&slots, symbol)
                .and_then(|index| slots.values[index].take())
                .or_else(|| {
                    frame
                        .restored
                        .as_ref()
                        .and_then(|locals| locals.borrow_mut().remove(&symbol))
                });
            if let Some(value) = value {
                slots.scoped_captures = true;
                slots
                    .shared
                    .insert(symbol, SharedBinding::new(frame.snapshot_id, value));
                return Ok(());
            }
            current = frame.lexical_parent();
        }
        Err("captured mutable binding is not initialized".into())
    }

    pub(crate) fn install_shared_binding(&mut self, symbol: SymbolId, binding: SharedBinding) {
        let mut slots = self.storage.slots.borrow_mut();
        if let Some(index) = self.resolve(&slots, symbol) {
            slots.values[index] = None;
        }
        if let Some(locals) = &self.restored {
            locals.borrow_mut().remove(&symbol);
        }
        slots.scoped_captures = true;
        slots.shared.insert(symbol, binding);
        slots.scope_symbols = None;
    }

    pub(crate) fn shared_bindings(&self) -> Vec<BindingIdentity> {
        let slots = self.storage.slots.borrow();
        let mut bindings = slots
            .shared
            .iter()
            .map(|(symbol, binding)| BindingIdentity {
                symbol: *symbol,
                id: binding.id,
                owner: binding.owner,
            })
            .collect::<Vec<_>>();
        bindings.sort_by_key(|binding| binding.symbol.0);
        bindings
    }

    pub(crate) fn may_retain_scoped_bindings(&self) -> bool {
        self.storage.slots.borrow().scoped_captures
    }

    pub(crate) fn mark_scoped_value(&self, symbol: SymbolId) -> bool {
        let mut slots = self.storage.slots.borrow_mut();
        let changed = !slots.scoped_captures;
        slots.scoped_captures = true;
        if !slots.shared.contains_key(&symbol) {
            slots.scoped_values.insert(symbol);
        }
        changed
    }

    pub(crate) fn has_expired_bindings(&self) -> bool {
        self.storage
            .slots
            .borrow()
            .shared
            .values()
            .any(SharedBinding::is_expired)
    }

    pub(crate) fn has_shared_bindings(&self) -> bool {
        !self.storage.slots.borrow().shared.is_empty()
    }

    pub(crate) fn scoped_value_escapes(
        &self,
        value: &InterpValue,
        keep: &HashSet<SymbolId>,
    ) -> bool {
        let ending = self
            .storage
            .slots
            .borrow()
            .shared
            .iter()
            .filter_map(|(symbol, binding)| {
                (binding.owner == self.snapshot_id && !keep.contains(symbol)).then_some(binding.id)
            })
            .collect::<HashSet<_>>();
        !ending.is_empty()
            && crate::value::graph::retains_bindings(
                crate::value::graph::Node::Value(value),
                |id| ending.contains(&id),
            )
    }

    pub(crate) fn scoped_local_escapes(&self, keep: &HashSet<SymbolId>) -> bool {
        if !self
            .storage
            .slots
            .borrow()
            .shared
            .iter()
            .any(|(symbol, binding)| binding.owner == self.snapshot_id && !keep.contains(symbol))
        {
            return false;
        }
        self.sorted_locals()
            .iter()
            .any(|(symbol, value)| keep.contains(symbol) && self.scoped_value_escapes(value, keep))
    }

    #[cfg(test)]
    pub(crate) fn from_snapshot(locals: Vec<(SymbolId, InterpValue)>) -> Result<Self, String> {
        Self::from_snapshot_with_generic_bindings(locals, super::GenericBindings::new())
    }

    pub(crate) fn from_snapshot_with_generic_bindings(
        locals: Vec<(SymbolId, InterpValue)>,
        bindings: super::GenericBindings,
    ) -> Result<Self, String> {
        let mut restored = HashMap::with_capacity(locals.len());
        for (symbol, value) in locals {
            if restored.insert(symbol, value).is_some() {
                return Err(format!(
                    "snapshot frame contains duplicate local symbol {}",
                    symbol.0
                ));
            }
        }
        let scoped_values = restored
            .iter()
            .filter_map(|(symbol, value)| {
                crate::value::graph::has_scoped_bindings(crate::value::graph::Node::Value(value))
                    .then_some(*symbol)
            })
            .collect::<HashSet<_>>();
        let snapshot_id = NEXT_FRAME_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Self {
            snapshot_id,
            residual_invocation: None,
            parent: None,
            type_bindings: bindings.shared(),
            storage: Rc::new(FrameStorage {
                layout: Arc::new(SlotLayoutTable::default()),
                slots: RefCell::new(SlotStorage {
                    owner: snapshot_id,
                    shared: HashMap::new(),
                    scoped_captures: !scoped_values.is_empty(),
                    scoped_values,
                    values: Vec::new(),
                    additional: HashMap::new(),
                    scope_symbols: None,
                    handler_activations: Default::default(),
                    exclusive_receivers: HashSet::new(),
                }),
            }),
            restored: Some(Rc::new(RefCell::new(RestoredStorage(restored)))),
        })
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
        (Rc::as_ptr(&self.storage).cast(), self.snapshot_id)
    }

    pub(crate) fn into_unique_values(self) -> FrameValues {
        let slots = Rc::try_unwrap(self.storage)
            .ok()
            .map(|slots| std::mem::take(&mut slots.slots.into_inner().values))
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

    pub(crate) fn fresh_snapshot_id() -> u64 {
        NEXT_FRAME_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn set_snapshot_id(&mut self, id: u64) -> Result<(), String> {
        if id == 0 {
            return Err("snapshot frame identity must be nonzero".into());
        }
        self.snapshot_id = id;
        self.storage.slots.borrow_mut().owner = id;
        NEXT_FRAME_ID.fetch_max(id + 1, std::sync::atomic::Ordering::Relaxed);
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

    pub(crate) fn receiver_is_held(&self, symbol: SymbolId) -> bool {
        let mut current = Some(self);
        while let Some(frame) = current {
            let slots = frame.storage.slots.borrow();
            if let Some(binding) = slots.shared.get(&symbol) {
                return binding.held();
            }
            if slots.exclusive_receivers.contains(&symbol) {
                return true;
            }
            if frame.resolve(&slots, symbol).is_some()
                || frame
                    .restored
                    .as_ref()
                    .is_some_and(|locals| locals.borrow().contains_key(&symbol))
            {
                return false;
            }
            current = frame.lexical_parent();
        }
        false
    }

    pub(crate) fn acquire_receiver(
        &self,
        symbol: SymbolId,
    ) -> Result<(super::ReceiverLease, InterpValue), String> {
        let mut current = Some(self);
        while let Some(frame) = current {
            let mut slots = frame.storage.slots.borrow_mut();
            if let Some(binding) = slots.shared.get(&symbol) {
                let value = binding.acquire()?;
                return Ok((super::ReceiverLease::new(frame.clone(), symbol), value));
            }
            if slots.exclusive_receivers.contains(&symbol) {
                return Err("updating receiver is already exclusively held".into());
            }
            let value = if let Some(slot) = frame.resolve(&slots, symbol) {
                slots.values[slot].clone()
            } else {
                frame
                    .restored
                    .as_ref()
                    .and_then(|locals| locals.borrow().get(&symbol).cloned())
            };
            if let Some(value) = value {
                slots.exclusive_receivers.insert(symbol);
                return Ok((super::ReceiverLease::new(frame.clone(), symbol), value));
            }
            current = frame.lexical_parent();
        }
        Err("updating receiver binding is not initialized".into())
    }

    pub(super) fn release_receiver(&self, symbol: SymbolId) {
        if let Some(binding) = self.storage.slots.borrow().shared.get(&symbol) {
            binding.release();
            return;
        }
        assert!(
            self.storage
                .slots
                .borrow_mut()
                .exclusive_receivers
                .remove(&symbol)
        );
    }

    pub fn get(&self, symbol: SymbolId) -> Option<InterpValue> {
        self.with_value(symbol, Clone::clone)
    }

    pub(crate) fn contains_value(&self, symbol: SymbolId) -> bool {
        self.with_value(symbol, |_| ()).is_some()
    }

    pub(crate) fn with_value<R>(
        &self,
        symbol: SymbolId,
        read: impl FnOnce(&InterpValue) -> R,
    ) -> Option<R> {
        let mut current = Some(self);
        while let Some(frame) = current {
            let slots = frame.storage.slots.borrow();
            if let Some(binding) = slots.shared.get(&symbol) {
                return binding.with_available_value(read);
            }
            if slots.exclusive_receivers.contains(&symbol) {
                return None;
            }
            if let Some(slot) = frame.resolve(&slots, symbol) {
                return slots.values[slot].as_ref().map(read);
            }
            if let Some(locals) = &frame.restored
                && let Some(value) = locals.borrow().get(&symbol)
            {
                return Some(read(value));
            }
            current = frame.lexical_parent();
        }
        None
    }

    pub fn insert(&mut self, symbol: SymbolId, value: InterpValue) {
        let mut slots = self.storage.slots.borrow_mut();
        if let Some(binding) = slots.shared.get(&symbol) {
            assert!(
                binding.update(|slot| *slot = value).is_some(),
                "captured binding is held or outside its owning scope"
            );
            return;
        }
        assert!(
            !slots.exclusive_receivers.contains(&symbol),
            "updating receiver is exclusively held"
        );
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
            let mut slots = frame.storage.slots.borrow_mut();
            if let Some(binding) = slots.shared.get(&symbol) {
                return binding.update(|slot| *slot = value).is_some();
            }
            if slots.exclusive_receivers.contains(&symbol) {
                return false;
            }
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
            let mut slots = frame.storage.slots.borrow_mut();
            if let Some(binding) = slots.shared.get(&symbol) {
                return binding.update(update);
            }
            if slots.exclusive_receivers.contains(&symbol) {
                return None;
            }
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
        let slots = self.storage.slots.borrow();
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
        symbols.extend(slots.shared.keys().copied());
        symbols
    }

    /// Reassignments preserve scope membership. Frame aliases share invalidation,
    /// while previously returned scope snapshots remain immutable.
    pub(crate) fn scope_symbols(&self) -> Rc<HashSet<SymbolId>> {
        let mut slots = self.storage.slots.borrow_mut();
        if let Some(cached) = &slots.scope_symbols {
            return cached.clone();
        }
        let symbols = Rc::new(self.collect_symbols(&slots));
        slots.scope_symbols = Some(symbols.clone());
        symbols
    }

    pub fn cleanup_to(&mut self, keep: &HashSet<SymbolId>) {
        let mut slots = self.storage.slots.borrow_mut();
        if slots
            .scope_symbols
            .as_ref()
            .is_some_and(|cached| std::ptr::eq(cached.as_ref(), keep))
        {
            return;
        }
        let owner = slots.owner;
        let shared_count = slots.shared.len();
        slots.shared.retain(|symbol, binding| {
            if keep.contains(symbol) {
                return true;
            }
            if binding.owner == owner {
                binding.expire();
            }
            false
        });
        let shared_removed = slots.shared.len() != shared_count;
        let SlotStorage {
            values, additional, ..
        } = &mut *slots;
        let mut removed = shared_removed;
        for (symbol, index) in self
            .storage
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
            let slots = frame.storage.slots.borrow();
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
            for (symbol, binding) in &slots.shared {
                if let Some(value) = binding.value() {
                    locals.entry(*symbol).or_insert(value);
                }
            }
            current = frame.lexical_parent();
        }
        locals.into_iter().collect()
    }

    pub(crate) fn try_map_locals<T, E>(
        &self,
        mut map: impl FnMut(SymbolId, bool, &InterpValue) -> Result<T, E>,
    ) -> Result<Vec<(SymbolId, T)>, E> {
        let slots = self.storage.slots.borrow();
        let restored = self.restored.as_ref().map(|locals| locals.borrow());
        let count = slots.values.iter().filter(|value| value.is_some()).count()
            + restored.as_ref().map_or(0, |locals| locals.len())
            + slots.shared.len();
        let mut output = Vec::with_capacity(count);
        for (symbol, index) in self.slot_symbols(&slots) {
            if let Some(value) = &slots.values[index] {
                output.push((symbol, map(symbol, false, value)?));
            }
        }
        if let Some(restored) = restored {
            for (symbol, value) in restored.iter() {
                output.push((*symbol, map(*symbol, false, value)?));
            }
        }
        for (symbol, binding) in &slots.shared {
            if let Some(value) = binding
                .visit(|value| map(*symbol, true, value))
                .transpose()?
            {
                output.push((*symbol, value));
            }
        }
        output.sort_unstable_by_key(|(symbol, _)| symbol.0);
        Ok(output)
    }

    pub(crate) fn try_visit_locals<E>(
        &self,
        mut visit: impl FnMut(&InterpValue) -> Result<(), E>,
    ) -> Result<(), E> {
        let slots = self.storage.slots.borrow();
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
        for binding in slots.shared.values() {
            binding.visit(&mut visit).transpose()?;
        }
        Ok(())
    }

    pub(crate) fn try_visit_scoped_locals<E>(
        &self,
        mut visit: impl FnMut(&InterpValue) -> Result<(), E>,
    ) -> Result<(), E> {
        let slots = self.storage.slots.borrow();
        for (symbol, index) in self.slot_symbols(&slots) {
            if slots.scoped_values.contains(&symbol)
                && let Some(value) = &slots.values[index]
            {
                visit(value)?;
            }
        }
        if let Some(restored) = &self.restored {
            for (symbol, value) in restored.borrow().iter() {
                if slots.scoped_values.contains(symbol) {
                    visit(value)?;
                }
            }
        }
        for binding in slots.shared.values() {
            binding.visit(&mut visit).transpose()?;
        }
        Ok(())
    }

    fn resolve(&self, slots: &SlotStorage, symbol: SymbolId) -> Option<usize> {
        self.storage
            .layout
            .resolve(symbol)
            .map(|slot| slot.0 as usize)
            .or_else(|| slots.additional.get(&symbol).copied())
    }

    fn slot_symbols<'a>(
        &'a self,
        slots: &'a SlotStorage,
    ) -> impl Iterator<Item = (SymbolId, usize)> + 'a {
        self.storage
            .layout
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
        let mut slots = self.storage.slots.borrow_mut();
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

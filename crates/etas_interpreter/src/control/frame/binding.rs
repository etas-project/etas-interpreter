use super::{InterpValue, Rc, RefCell, SymbolId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct BindingIdentity {
    pub symbol: SymbolId,
    pub id: u64,
    pub owner: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct SharedBinding {
    pub id: u64,
    pub owner: u64,
    state: Rc<RefCell<BindingState>>,
}

#[derive(Debug)]
struct BindingState {
    value: Option<InterpValue>,
    held: bool,
}

impl Drop for BindingState {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            crate::value::release_value(value);
        }
    }
}

static NEXT_BINDING_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl SharedBinding {
    pub(crate) fn new(owner: u64, value: InterpValue) -> Self {
        let id = NEXT_BINDING_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            id,
            owner,
            state: Rc::new(RefCell::new(BindingState {
                value: Some(value),
                held: false,
            })),
        }
    }

    pub(crate) fn value(&self) -> Option<InterpValue> {
        let state = self.state.borrow();
        (!state.held).then(|| state.value.clone()).flatten()
    }

    pub(super) fn is_expired(&self) -> bool {
        self.state.borrow().value.is_none()
    }

    pub(super) fn held(&self) -> bool {
        self.state.borrow().held
    }

    pub(super) fn acquire(&self) -> Result<InterpValue, String> {
        let mut state = self.state.borrow_mut();
        if state.held {
            return Err("updating receiver is already exclusively held".into());
        }
        let value = state
            .value
            .clone()
            .ok_or("captured binding is outside its owning scope")?;
        state.held = true;
        Ok(value)
    }

    pub(super) fn release(&self) {
        let mut state = self.state.borrow_mut();
        assert!(state.held);
        state.held = false;
    }

    pub(super) fn update<R>(&self, update: impl FnOnce(&mut InterpValue) -> R) -> Option<R> {
        let mut state = self.state.borrow_mut();
        if state.held {
            return None;
        }
        state.value.as_mut().map(update)
    }

    pub(super) fn visit<R>(&self, visit: impl FnOnce(&InterpValue) -> R) -> Option<R> {
        let state = self.state.borrow();
        state.value.as_ref().map(visit)
    }

    pub(super) fn expire(&self) {
        let value = self.state.borrow_mut().value.take();
        if let Some(value) = value {
            crate::value::release_value(value);
        }
    }
}

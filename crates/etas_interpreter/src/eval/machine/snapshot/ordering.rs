use std::{cell::RefCell, rc::Rc};

use crate::value::{CheckedExecutionProgram, OrderedMapValue, OrderedSetValue};

#[derive(Clone)]
pub(crate) enum RestoredOrderedValue {
    Map(OrderedMapValue),
    Set(OrderedSetValue),
}

pub(crate) struct OrderedRestoreCheck {
    pub program: CheckedExecutionProgram,
    pub value: RestoredOrderedValue,
}

/// Restore records postorder checks while rebuilding data. The driver consumes
/// every check before it can publish or poll the restored execution stack.
#[derive(Clone, Default)]
pub(crate) struct OrderedRestoreChecks(Rc<RefCell<Vec<OrderedRestoreCheck>>>);

impl OrderedRestoreChecks {
    pub(super) fn register(
        &self,
        program: CheckedExecutionProgram,
        value: RestoredOrderedValue,
    ) -> Result<(), String> {
        self.0
            .try_borrow_mut()
            .map_err(|_| "ordered restoration is already being inspected")?
            .push(OrderedRestoreCheck { program, value });
        Ok(())
    }

    pub(crate) fn take(&self) -> Result<Vec<OrderedRestoreCheck>, String> {
        Ok(std::mem::take(&mut *self.0.try_borrow_mut().map_err(
            |_| "ordered restoration is already being inspected",
        )?))
    }
}

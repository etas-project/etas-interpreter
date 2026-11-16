use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use crate::eval::{host_completion::HostCompletionContext, machine::MachineInput};
use etas_host::execution::{CancellationReason, ExecutionScope, OperationId};

/// The supervisor retains the operation, while the language wait owns this
/// lease. Dropping a Handler continuation cancels only its abandoned operation.
#[derive(Debug)]
pub(crate) struct HostWait {
    state: Rc<HostOperation>,
}

pub(in crate::driver) struct HostOperationScope {
    execution: ExecutionScope,
    finished: Cell<bool>,
    operations: Rc<Cell<usize>>,
}

impl HostOperationScope {
    pub(in crate::driver) fn new(
        parent: &mut crate::eval::TaskLocalState,
    ) -> Result<Self, etas_host::HostError> {
        let execution = parent.execution.child()?;
        let operations = parent
            .host_operations
            .get_or_insert_with(|| Rc::new(Cell::new(0)))
            .clone();
        operations.set(operations.get() + 1);
        let scope = Self {
            execution,
            finished: Cell::new(false),
            operations,
        };
        scope.execution.claim_body()?;
        Ok(scope)
    }
    pub(in crate::driver) fn execution(&self) -> &ExecutionScope {
        &self.execution
    }
    pub(in crate::driver) fn finish(&self) -> Result<(), etas_host::HostError> {
        if !self.finished.replace(true) {
            self.execution.finish_body(true)?;
            self.operations.set(self.operations.get() - 1);
        }
        Ok(())
    }
}

impl Drop for HostOperationScope {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

pub(in crate::driver) struct HostOperation {
    scope: HostOperationScope,
    id: OperationId,
    pub(in crate::driver) context: HostCompletionContext,
    waiting: Cell<bool>,
    input: RefCell<Option<MachineInput>>,
}

impl std::fmt::Debug for HostOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostOperation")
            .field("id", &self.id)
            .field("settled", &self.scope.finished.get())
            .finish()
    }
}

impl HostOperation {
    pub(in crate::driver) fn new(
        scope: HostOperationScope,
        id: OperationId,
        context: HostCompletionContext,
    ) -> Rc<Self> {
        Rc::new(Self {
            scope,
            id,
            context,
            waiting: Cell::new(false),
            input: RefCell::new(None),
        })
    }
    pub(in crate::driver) fn wait(self: &Rc<Self>) -> HostWait {
        assert!(
            !self.waiting.replace(true),
            "Host operation has one language wait"
        );
        HostWait {
            state: self.clone(),
        }
    }
    pub(in crate::driver) fn finish_native(&self) -> Result<(), etas_host::HostError> {
        self.scope.finish()
    }
    pub(in crate::driver) fn is_waiting(&self) -> bool {
        self.waiting.get()
    }
    pub(in crate::driver) fn complete_input(&self, input: MachineInput) {
        if self.waiting.get() {
            assert!(
                self.input.borrow_mut().replace(input).is_none(),
                "Host reply is committed once"
            );
        }
    }
    pub(in crate::driver) fn id(&self) -> OperationId {
        self.id
    }
}

impl HostWait {
    pub(crate) fn operation_id(&self) -> OperationId {
        self.state.id
    }
    pub(crate) fn input(&self) -> Option<MachineInput> {
        self.state.input.borrow().clone()
    }
    pub(crate) fn take_input(&self) -> Option<MachineInput> {
        self.state.input.borrow_mut().take()
    }
}

impl Drop for HostWait {
    fn drop(&mut self) {
        self.state.waiting.set(false);
        if !self.state.scope.finished.get() {
            let _ = self
                .state
                .scope
                .execution
                .cancel_source()
                .stop(CancellationReason::OwnerDropped);
        }
    }
}

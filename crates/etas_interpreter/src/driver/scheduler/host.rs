use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use super::{HostOperation, HostWait, TaskHandle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::driver) enum HostOwner {
    Root,
    Task(TaskHandle),
}

pub(in crate::driver) enum CompletedTaskHost {
    Console(crate::driver::dispatch::console::CompletedConsoleDispatch),
    Command(crate::driver::dispatch::command::CompletedCommandDispatch),
    Host(crate::driver::dispatch::host::CompletedHostBoundaryDispatch),
    Tool(crate::driver::dispatch::tool::CompletedToolHostDispatch),
    Model(crate::driver::dispatch::model::CompletedModelHostDispatch),
    Approval(crate::driver::dispatch::approval::CompletedApprovalDispatch),
    Compiler(crate::driver::dispatch::compiler::CompletedCompilerDispatch),
    Memory(crate::driver::dispatch::memory::CompletedMemoryDispatch),
}

struct PendingTaskHost<'host> {
    owner: HostOwner,
    operation: Rc<HostOperation>,
    future: Pin<Box<dyn Future<Output = CompletedTaskHost> + 'host>>,
}

pub(in crate::driver) struct ReadyTaskHost {
    pub(in crate::driver) owner: HostOwner,
    pub(in crate::driver) operation: Rc<HostOperation>,
    pub(in crate::driver) completed: CompletedTaskHost,
}

pub(in crate::driver) struct TaskHostReactor<'host> {
    pending: VecDeque<PendingTaskHost<'host>>,
}

impl<'host> TaskHostReactor<'host> {
    pub(in crate::driver) fn new() -> Self {
        Self {
            pending: VecDeque::new(),
        }
    }

    pub(in crate::driver) fn start_console(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::console::PendingConsoleDispatch<'host>,
    ) -> HostWait {
        let operation = pending.operation_state();
        let wait = operation.wait();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Console(pending.poll().await) }),
        });
        wait
    }

    pub(in crate::driver) fn start_command(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::command::PendingCommandDispatch<'host>,
    ) -> HostWait {
        let operation = pending.operation_state();
        let wait = operation.wait();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Command(pending.poll().await) }),
        });
        wait
    }

    pub(in crate::driver) fn start_host(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::host::PendingHostBoundaryDispatch<'host>,
    ) -> HostWait {
        let operation = pending.operation_state();
        let wait = operation.wait();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Host(pending.poll().await) }),
        });
        wait
    }

    pub(in crate::driver) fn start_tool(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::tool::PendingToolHostDispatch<'host>,
    ) -> HostWait {
        let operation = pending.operation_state();
        let wait = operation.wait();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Tool(pending.poll().await) }),
        });
        wait
    }

    pub(in crate::driver) fn start_model(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::model::PendingModelHostDispatch<'host>,
    ) -> HostWait {
        let operation = pending.operation_state();
        let wait = operation.wait();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Model(pending.poll().await) }),
        });
        wait
    }

    pub(in crate::driver) fn start_approval(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::approval::PendingApprovalDispatch<'host>,
    ) -> HostWait {
        let operation = pending.operation_state();
        let wait = operation.wait();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Approval(pending.poll().await) }),
        });
        wait
    }

    pub(in crate::driver) fn start_compiler(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::compiler::PendingCompilerDispatch<'host>,
    ) -> HostWait {
        let operation = pending.operation_state();
        let wait = operation.wait();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Compiler(pending.poll().await) }),
        });
        wait
    }

    pub(in crate::driver) fn start_memory(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::memory::PendingMemoryDispatch<'host>,
    ) -> HostWait {
        let operation = pending.operation_state();
        let wait = operation.wait();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Memory(pending.poll().await) }),
        });
        wait
    }

    pub(in crate::driver) async fn poll_ready(&mut self) -> Option<ReadyTaskHost> {
        std::future::poll_fn(|cx| Poll::Ready(self.poll_one(cx))).await
    }

    pub(in crate::driver) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(in crate::driver) async fn wait_next(&mut self) -> Option<ReadyTaskHost> {
        std::future::poll_fn(|cx| {
            if let Some(completed) = self.poll_one(cx) {
                Poll::Ready(Some(completed))
            } else if self.pending.is_empty() {
                Poll::Ready(None)
            } else {
                Poll::Pending
            }
        })
        .await
    }

    fn poll_one(&mut self, cx: &mut Context<'_>) -> Option<ReadyTaskHost> {
        let pending = self.pending.len();
        for _ in 0..pending {
            let mut operation = self.pending.pop_front().expect("pending host operation");
            match operation.future.as_mut().poll(cx) {
                Poll::Ready(completed) => {
                    return Some(ReadyTaskHost {
                        owner: operation.owner,
                        operation: operation.operation,
                        completed,
                    });
                }
                Poll::Pending => self.pending.push_back(operation),
            }
        }
        None
    }
}

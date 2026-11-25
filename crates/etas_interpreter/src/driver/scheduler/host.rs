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
    Authorization(crate::driver::dispatch::authorization::CompletedAuthorizationDispatch),
    Managed(crate::driver::dispatch::managed_agent::CompletedManagedAuthorizationDispatch),
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

pub(in crate::driver) enum ReadyTaskHost {
    Operation {
        owner: HostOwner,
        operation: Rc<HostOperation>,
        completed: CompletedTaskHost,
    },
    Cleanup {
        task: TaskHandle,
        report: Result<etas_host::execution::TerminationReport, etas_host::HostError>,
    },
}

struct PendingCleanup<'host> {
    task: TaskHandle,
    future: Pin<
        Box<
            dyn Future<
                    Output = Result<etas_host::execution::TerminationReport, etas_host::HostError>,
                > + 'host,
        >,
    >,
}

pub(in crate::driver) struct TaskHostReactor<'host> {
    pending: VecDeque<PendingTaskHost<'host>>,
    cleanup: VecDeque<PendingCleanup<'host>>,
}

impl<'host> TaskHostReactor<'host> {
    pub(in crate::driver) fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            cleanup: VecDeque::new(),
        }
    }

    pub(in crate::driver) fn drain_resources(
        &mut self,
        task: TaskHandle,
        execution: etas_host::execution::ExecutionScope,
    ) {
        self.cleanup.push_back(PendingCleanup {
            task,
            future: Box::pin(async move { execution.join().await }),
        });
    }

    pub(in crate::driver) fn start_authorization(
        &mut self,
        owner: HostOwner,
        pending: Box<crate::driver::dispatch::authorization::PendingAuthorizationDispatch<'host>>,
    ) -> HostWait {
        let operation = pending.operation_state();
        let wait = operation.wait();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Authorization(pending.poll().await) }),
        });
        wait
    }

    pub(in crate::driver) fn start_authorized(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::authorization::AuthorizedHostDispatch<'host>,
    ) -> HostWait {
        use crate::driver::dispatch::authorization::AuthorizedHostDispatch as Pending;
        match pending {
            Pending::Console(pending) => self.start_console(owner, pending),
            Pending::Command(pending) => self.start_command(owner, pending),
            Pending::Memory(pending) => self.start_memory(owner, pending),
            Pending::Host(pending) => self.start_host(owner, pending),
            Pending::Model(pending) => self.start_model(owner, pending),
            Pending::Tool(pending) => self.start_tool(owner, pending),
            Pending::Compiler(pending) => self.start_compiler(owner, pending),
        }
    }

    pub(in crate::driver) fn start_managed(
        &mut self,
        owner: HostOwner,
        pending: Box<
            crate::driver::dispatch::managed_agent::PendingManagedAuthorizationDispatch<'host>,
        >,
    ) -> HostWait {
        let operation = pending.operation_state();
        let wait = operation.wait();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Managed(pending.poll().await) }),
        });
        wait
    }

    pub(in crate::driver) fn start_console(
        &mut self,
        owner: HostOwner,
        pending: Box<crate::driver::dispatch::console::PendingConsoleDispatch<'host>>,
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
        pending: Box<crate::driver::dispatch::command::PendingCommandDispatch<'host>>,
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
        pending: Box<crate::driver::dispatch::host::PendingHostBoundaryDispatch<'host>>,
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
        pending: Box<crate::driver::dispatch::tool::PendingToolHostDispatch<'host>>,
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
        pending: Box<crate::driver::dispatch::model::PendingModelHostDispatch<'host>>,
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
        pending: Box<crate::driver::dispatch::approval::PendingApprovalDispatch<'host>>,
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
        pending: Box<crate::driver::dispatch::compiler::PendingCompilerDispatch<'host>>,
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
        pending: Box<crate::driver::dispatch::memory::PendingMemoryDispatch<'host>>,
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
        !self.pending.is_empty() || !self.cleanup.is_empty()
    }

    /// A checkpoint preserves language-child joins. Waiting for those joins
    /// here would wait for children whose execution the barrier has paused.
    pub(in crate::driver) fn has_checkpoint_work(
        &self,
        scheduler: &super::TaskScheduler,
    ) -> Result<bool, String> {
        if !self.pending.is_empty() {
            return Ok(true);
        }
        let tasks = scheduler.checkpoint_tasks();
        for cleanup in &self.cleanup {
            let task = tasks
                .get(&cleanup.task.task)
                .expect("resource owner exists");
            if !super::TaskScheduler::checkpoint_waiting_for_children(task, tasks)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(in crate::driver) async fn wait_next(&mut self) -> Option<ReadyTaskHost> {
        std::future::poll_fn(|cx| {
            if let Some(completed) = self.poll_one(cx) {
                Poll::Ready(Some(completed))
            } else if !self.has_pending() {
                Poll::Ready(None)
            } else {
                Poll::Pending
            }
        })
        .await
    }

    fn poll_one(&mut self, cx: &mut Context<'_>) -> Option<ReadyTaskHost> {
        let cleanup = self.cleanup.len();
        for _ in 0..cleanup {
            let mut owner = self.cleanup.pop_front().expect("pending resource owner");
            match owner.future.as_mut().poll(cx) {
                Poll::Ready(report) => {
                    return Some(ReadyTaskHost::Cleanup {
                        task: owner.task,
                        report,
                    });
                }
                Poll::Pending => self.cleanup.push_back(owner),
            }
        }
        let pending = self.pending.len();
        for _ in 0..pending {
            let mut operation = self.pending.pop_front().expect("pending host operation");
            match operation.future.as_mut().poll(cx) {
                Poll::Ready(completed) => {
                    return Some(ReadyTaskHost::Operation {
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

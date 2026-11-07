use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use super::{TaskHandle, TaskId};

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
}

struct PendingTaskHost<'host> {
    owner: HostOwner,
    operation: etas_host::execution::OperationId,
    future: Pin<Box<dyn Future<Output = CompletedTaskHost> + 'host>>,
}

pub(in crate::driver) struct TaskHostReactor<'host> {
    pending: VecDeque<PendingTaskHost<'host>>,
    completed: BTreeMap<TaskId, CompletedTaskHost>,
    root_completed: Option<CompletedTaskHost>,
}

impl<'host> TaskHostReactor<'host> {
    pub(in crate::driver) fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            completed: BTreeMap::new(),
            root_completed: None,
        }
    }

    pub(in crate::driver) fn start_console(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::console::PendingConsoleDispatch<'host>,
    ) -> etas_host::execution::OperationId {
        let operation = pending.operation_id();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Console(pending.poll().await) }),
        });
        operation
    }

    pub(in crate::driver) fn start_command(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::command::PendingCommandDispatch<'host>,
    ) -> etas_host::execution::OperationId {
        let operation = pending.operation_id();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Command(pending.poll().await) }),
        });
        operation
    }

    pub(in crate::driver) fn start_host(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::host::PendingHostBoundaryDispatch<'host>,
    ) -> etas_host::execution::OperationId {
        let operation = pending.operation_id();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Host(pending.poll().await) }),
        });
        operation
    }

    pub(in crate::driver) fn start_tool(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::tool::PendingToolHostDispatch<'host>,
    ) -> etas_host::execution::OperationId {
        let operation = pending.operation_id();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Tool(pending.poll().await) }),
        });
        operation
    }

    pub(in crate::driver) fn start_model(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::model::PendingModelHostDispatch<'host>,
    ) -> etas_host::execution::OperationId {
        let operation = pending.operation_id();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Model(pending.poll().await) }),
        });
        operation
    }

    pub(in crate::driver) fn start_approval(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::approval::PendingApprovalDispatch<'host>,
    ) -> etas_host::execution::OperationId {
        let operation = pending.operation_id();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Approval(pending.poll().await) }),
        });
        operation
    }

    pub(in crate::driver) fn start_compiler(
        &mut self,
        owner: HostOwner,
        pending: crate::driver::dispatch::compiler::PendingCompilerDispatch<'host>,
    ) -> etas_host::execution::OperationId {
        let operation = pending.operation_id();
        self.pending.push_back(PendingTaskHost {
            owner,
            operation,
            future: Box::pin(async move { CompletedTaskHost::Compiler(pending.poll().await) }),
        });
        operation
    }

    pub(in crate::driver) fn take_completed(
        &mut self,
        handle: TaskHandle,
    ) -> Option<CompletedTaskHost> {
        self.completed.remove(&handle.task)
    }

    pub(in crate::driver) fn take_root_completed(&mut self) -> Option<CompletedTaskHost> {
        self.root_completed.take()
    }

    pub(in crate::driver) async fn poll_ready(
        &mut self,
    ) -> Option<(HostOwner, etas_host::execution::OperationId)> {
        std::future::poll_fn(|cx| Poll::Ready(self.poll_one(cx))).await
    }

    pub(in crate::driver) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(in crate::driver) async fn wait_next(
        &mut self,
    ) -> Option<(HostOwner, etas_host::execution::OperationId)> {
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

    fn poll_one(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Option<(HostOwner, etas_host::execution::OperationId)> {
        let pending = self.pending.len();
        for _ in 0..pending {
            let mut operation = self.pending.pop_front().expect("pending host operation");
            match operation.future.as_mut().poll(cx) {
                Poll::Ready(completed) => {
                    match operation.owner {
                        HostOwner::Root => {
                            assert!(
                                self.root_completed.replace(completed).is_none(),
                                "root cannot own two completed host operations"
                            );
                        }
                        HostOwner::Task(handle) => {
                            assert!(
                                self.completed.insert(handle.task, completed).is_none(),
                                "task cannot own two completed host operations"
                            );
                        }
                    }
                    return Some((operation.owner, operation.operation));
                }
                Poll::Pending => self.pending.push_back(operation),
            }
        }
        None
    }
}

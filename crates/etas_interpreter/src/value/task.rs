#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskHandleValue {
    pub(crate) handle: crate::driver::scheduler::TaskHandle,
}

impl TaskHandleValue {
    pub(crate) fn new(handle: crate::driver::scheduler::TaskHandle) -> Self {
        Self { handle }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchedulerHandleValue {
    pub(crate) owner: crate::driver::scheduler::TaskScopeId,
}

impl SchedulerHandleValue {
    pub(crate) fn new(owner: crate::driver::scheduler::TaskScopeId) -> Self {
        Self { owner }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContinuationTokenValue {
    pub(crate) owner: crate::driver::scheduler::TaskScopeId,
    pub(crate) continuation: crate::driver::scheduler::ContinuationId,
}

impl ContinuationTokenValue {
    pub(crate) fn new(
        owner: crate::driver::scheduler::TaskScopeId,
        continuation: crate::driver::scheduler::ContinuationId,
    ) -> Self {
        Self {
            owner,
            continuation,
        }
    }
}

mod capsule;
mod completion;
mod inline;
mod worker_pool;

use std::num::NonZeroUsize;

pub(crate) use capsule::{
    CpuBudgetLease, CpuCallableId, CpuTaskCapsule, FrozenValue, PreparedCpuProgram,
};
pub(crate) use completion::{
    CpuBudgetDelta, CpuExecutionFailure, CpuJobId, CpuTaskCompletion, CpuTaskMetrics,
};
pub(crate) use inline::InlineCpuExecutor;
pub(crate) use worker_pool::WorkerPoolCpuExecutor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CpuSubmitError {
    ExecutorStopped,
}

pub(crate) trait CpuExecutor: Send + Sync {
    fn capacity(&self) -> NonZeroUsize;

    fn submit(&self, job: CpuTaskCapsule) -> Result<CpuJobId, CpuSubmitError>;

    fn try_completion(&self) -> Option<CpuTaskCompletion>;

    fn completion_signal(&self) -> &tokio::sync::Notify;

    fn cancel(&self, job: CpuJobId, reason: etas_host::execution::CancellationReason);
}

pub(crate) fn new_executor(threads: NonZeroUsize) -> std::sync::Arc<dyn CpuExecutor> {
    if threads.get() == 1 {
        std::sync::Arc::new(InlineCpuExecutor::new())
    } else {
        std::sync::Arc::new(WorkerPoolCpuExecutor::new(threads))
    }
}

#[cfg(test)]
mod send_assertions {
    use super::*;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    #[test]
    fn cpu_boundary_types_are_thread_safe_without_unsafe_impls() {
        assert_send::<CpuTaskCapsule>();
        assert_send::<CpuTaskCompletion>();
        assert_sync::<WorkerPoolCpuExecutor>();
    }
}

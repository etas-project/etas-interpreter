mod capsule;
mod completion;
mod inline;
mod worker_pool;

use std::num::NonZeroUsize;

#[cfg(test)]
pub(crate) use capsule::program_build_count;
pub(crate) use capsule::{
    CpuBudgetLease, CpuCallableId, CpuRuntimeContract, CpuTaskCapsule, FrozenValue,
    PreparedCpuProgram,
};
pub(crate) use completion::{
    CpuBudgetDelta, CpuContractViolation, CpuExecutionFailure, CpuJobId, CpuTaskCompletion,
    CpuTaskMetrics,
};
pub(crate) use inline::InlineCpuExecutor;
pub(crate) use worker_pool::CpuWorkerHealth;
pub(crate) use worker_pool::WorkerPoolCpuExecutor;
#[cfg(test)]
pub(crate) use worker_pool::take_test_pools;

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

    fn health(&self) -> Vec<CpuWorkerHealth> {
        Vec::new()
    }
}

pub(crate) fn new_executor(
    threads: NonZeroUsize,
) -> Result<std::sync::Arc<dyn CpuExecutor>, std::io::Error> {
    if threads.get() == 1 {
        Ok(std::sync::Arc::new(InlineCpuExecutor::new()))
    } else {
        Ok(std::sync::Arc::new(WorkerPoolCpuExecutor::new(threads)?))
    }
}

#[cfg(test)]
std::thread_local! {
    static PANIC_NEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn panic_next_task() {
    PANIC_NEXT.set(true);
}

#[cfg(test)]
pub(crate) fn take_test_panic() -> bool {
    PANIC_NEXT.replace(false)
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

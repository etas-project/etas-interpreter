#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct CpuJobId(pub(crate) u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CpuBudgetDelta {
    pub(crate) steps: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CpuExecutionFailure {
    Cancelled(etas_host::execution::CancellationReason),
    ExecutionFault(CpuFaultSnapshot),
    ContractViolation(CpuContractViolation),
    Infrastructure {
        worker: crate::orchestration::CpuWorkerId,
        message: String,
    },
}

// ExecutionFault is already an owned Send-safe diagnostic snapshot, including
// notes. Reuse it so crossing the worker boundary cannot discard information.
pub(crate) type CpuFaultSnapshot = crate::control::ExecutionFault;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CpuContractViolation {
    NonFreezableResult,
    EffectBoundary(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CpuTaskMetrics {
    pub(crate) worker: crate::orchestration::CpuWorkerId,
    pub(crate) dispatch_sequence: u64,
    pub(crate) thread: Option<std::thread::ThreadId>,
    pub(crate) elapsed: std::time::Duration,
    pub(crate) finished_at: std::time::Instant,
    pub(crate) timings: crate::api::CpuTaskTimings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CpuTaskCompletion {
    pub(crate) job: CpuJobId,
    pub(crate) result: Result<super::FrozenValue, CpuExecutionFailure>,
    pub(crate) consumed_budget: CpuBudgetDelta,
    pub(crate) metrics: CpuTaskMetrics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct CpuJobId(pub(crate) u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CpuBudgetDelta {
    pub(crate) steps: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CpuExecutionFailure {
    Cancelled(etas_host::execution::CancellationReason),
    Fault(String),
    EffectBoundary(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CpuTaskMetrics {
    pub(crate) dispatch_sequence: u64,
    pub(crate) thread: std::thread::ThreadId,
    pub(crate) elapsed: std::time::Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CpuTaskCompletion {
    pub(crate) job: CpuJobId,
    pub(crate) result: Result<super::FrozenValue, CpuExecutionFailure>,
    pub(crate) consumed_budget: CpuBudgetDelta,
    pub(crate) metrics: CpuTaskMetrics,
}

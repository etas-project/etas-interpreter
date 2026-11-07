use etas_core::{AnalysisDiagnosticCode, Span};

use crate::{api::ExecutionLimits, control::ExecutionFault};
use etas_host::execution::{CancelSignal, CancellationCause};

const TIME_BUDGET_CHECK_INTERVAL: u64 = 64;
const WORK_QUANTUM: u64 = 256;

pub(crate) enum SafePointDecision {
    Continue,
    Yield,
    Cancelled(CancellationCause),
    Fault(ExecutionFault),
}

#[cfg(test)]
#[path = "safe_point_tests.rs"]
mod tests;

pub(super) struct ExecutionSafePointScheduler {
    steps: std::sync::Arc<std::sync::atomic::AtomicU64>,
    local_steps: u64,
    quantum_work: u64,
}

impl ExecutionSafePointScheduler {
    pub(super) fn new(consumed_steps: u64) -> Self {
        Self {
            steps: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(consumed_steps)),
            local_steps: 0,
            quantum_work: 0,
        }
    }

    pub(super) fn with_ledger(steps: std::sync::Arc<std::sync::atomic::AtomicU64>) -> Self {
        Self {
            steps,
            local_steps: 0,
            quantum_work: 0,
        }
    }

    pub(super) fn ledger(&self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
        self.steps.clone()
    }

    pub(super) fn consumed_steps(&self) -> u64 {
        self.steps.load(std::sync::atomic::Ordering::Acquire)
    }

    pub(super) fn local_consumed_steps(&self) -> u64 {
        self.local_steps
    }

    pub(super) fn observe(
        &mut self,
        cancellation: &CancelSignal,
        budget: &etas_host::ExecutionBudget,
        span: Span,
    ) -> SafePointDecision {
        // The token is the cheap observation path; read the shared scope registry
        // only after cancellation has actually been published.
        if cancellation.is_cancelled() {
            match cancellation.cause() {
                Ok(Some(cause)) => return SafePointDecision::Cancelled(cause),
                Err(error) => {
                    return SafePointDecision::Fault(ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        error.to_string(),
                    ));
                }
                Ok(None) => {}
            }
        }
        if self.quantum_work >= WORK_QUANTUM {
            self.quantum_work = 0;
            // Check on every quantum, including after resuming from a Host wait.
            if let Err(fault) = Self::check_time(budget, span) {
                return SafePointDecision::Fault(fault);
            }
            return SafePointDecision::Yield;
        }
        SafePointDecision::Continue
    }

    fn check_time(budget: &etas_host::ExecutionBudget, span: Span) -> Result<(), ExecutionFault> {
        budget.check_time().map_err(|error| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                format!(
                    "interpreter execution exhausted the run-owned wall-time budget: {}",
                    error.message
                ),
            )
        })
    }

    pub(super) fn consume(
        &mut self,
        limits: ExecutionLimits,
        budget: &etas_host::ExecutionBudget,
        span: Span,
    ) -> Result<(), ExecutionFault> {
        let mut consumed = self.steps.load(std::sync::atomic::Ordering::Acquire);
        loop {
            if let Some(max_steps) = limits.max_steps
                && consumed >= max_steps.get()
            {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::UnhandledRuntimeError,
                    span,
                    format!(
                        "maximum interpreter execution steps ({max_steps}) exceeded; this usually indicates unbounded computation"
                    ),
                ));
            }
            if consumed % TIME_BUDGET_CHECK_INTERVAL == 0 {
                Self::check_time(budget, span)?;
            }
            match self.steps.compare_exchange_weak(
                consumed,
                consumed.saturating_add(1),
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => consumed = actual,
            }
        }
        self.local_steps = self.local_steps.saturating_add(1);
        self.quantum_work = self.quantum_work.saturating_add(1);
        Ok(())
    }
}

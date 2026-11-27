use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

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

enum StepLedger {
    Local(u64),
    Shared(Arc<AtomicU64>),
}

pub(super) struct ExecutionSafePointScheduler {
    steps: StepLedger,
    local_steps: u64,
    quantum_work: u64,
}

impl ExecutionSafePointScheduler {
    pub(super) fn new(consumed_steps: u64) -> Self {
        Self {
            steps: StepLedger::Local(consumed_steps),
            local_steps: 0,
            quantum_work: 0,
        }
    }

    pub(super) fn with_ledger(steps: Arc<AtomicU64>) -> Self {
        Self {
            steps: StepLedger::Shared(steps),
            local_steps: 0,
            quantum_work: 0,
        }
    }

    pub(super) fn ledger(&mut self) -> Arc<AtomicU64> {
        match &self.steps {
            StepLedger::Shared(steps) => steps.clone(),
            StepLedger::Local(consumed) => {
                let steps = Arc::new(AtomicU64::new(*consumed));
                self.steps = StepLedger::Shared(steps.clone());
                steps
            }
        }
    }

    pub(super) fn consumed_steps(&self) -> u64 {
        match &self.steps {
            StepLedger::Local(steps) => *steps,
            StepLedger::Shared(steps) => steps.load(Ordering::Acquire),
        }
    }

    pub(super) fn local_consumed_steps(&self) -> u64 {
        self.local_steps
    }

    pub(super) fn observe(
        &mut self,
        cancellation: &CancelSignal,
        budget: impl FnOnce() -> etas_host::ExecutionBudget,
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
            if let Err(fault) = Self::check_time(&budget(), span) {
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
        budget: impl Fn() -> etas_host::ExecutionBudget,
        span: Span,
    ) -> Result<(), ExecutionFault> {
        self.consume_work(limits, budget, span, 1)
    }

    pub(super) fn consume_work(
        &mut self,
        limits: ExecutionLimits,
        budget: impl Fn() -> etas_host::ExecutionBudget,
        span: Span,
        work: u64,
    ) -> Result<(), ExecutionFault> {
        let advance = |consumed: u64| {
            if let Some(max_steps) = limits.max_steps
                && work > max_steps.get().saturating_sub(consumed)
            {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::UnhandledRuntimeError,
                    span,
                    format!(
                        "maximum interpreter execution steps ({max_steps}) exceeded; this usually indicates unbounded computation"
                    ),
                ));
            }
            if consumed % TIME_BUDGET_CHECK_INTERVAL == 0
                || work > TIME_BUDGET_CHECK_INTERVAL - consumed % TIME_BUDGET_CHECK_INTERVAL
            {
                Self::check_time(&budget(), span)?;
            }
            Ok(consumed.saturating_add(work))
        };
        match &mut self.steps {
            StepLedger::Local(consumed) => *consumed = advance(*consumed)?,
            StepLedger::Shared(steps) => {
                let mut consumed = steps.load(Ordering::Acquire);
                loop {
                    match steps.compare_exchange_weak(
                        consumed,
                        advance(consumed)?,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => break,
                        Err(actual) => consumed = actual,
                    }
                }
            }
        }
        self.local_steps = self.local_steps.saturating_add(work);
        self.quantum_work = self.quantum_work.saturating_add(work);
        Ok(())
    }
}

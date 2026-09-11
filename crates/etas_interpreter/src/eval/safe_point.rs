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
    steps: u64,
    quantum_work: u64,
}

impl ExecutionSafePointScheduler {
    pub(super) fn new(consumed_steps: u64) -> Self {
        Self {
            steps: consumed_steps,
            quantum_work: 0,
        }
    }

    pub(super) fn consumed_steps(&self) -> u64 {
        self.steps
    }

    pub(super) fn observe(
        &mut self,
        cancellation: &CancelSignal,
        budget: &etas_host::ExecutionBudget,
        span: Span,
    ) -> SafePointDecision {
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
        if let Some(max_steps) = limits.max_steps
            && self.steps >= max_steps.get()
        {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                format!(
                    "maximum interpreter execution steps ({max_steps}) exceeded; this usually indicates unbounded computation"
                ),
            ));
        }
        if self.steps % TIME_BUDGET_CHECK_INTERVAL == 0 {
            Self::check_time(budget, span)?;
        }
        self.steps = self.steps.saturating_add(1);
        self.quantum_work = self.quantum_work.saturating_add(1);
        Ok(())
    }
}

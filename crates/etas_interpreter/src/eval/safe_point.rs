use etas_core::{AnalysisDiagnosticCode, Span};

use crate::{api::ExecutionLimits, control::ExecutionFault};

const TIME_BUDGET_CHECK_INTERVAL: u64 = 64;

pub(super) struct ExecutionSafePointScheduler {
    steps: u64,
}

impl ExecutionSafePointScheduler {
    pub(super) fn new(consumed_steps: u64) -> Self {
        Self {
            steps: consumed_steps,
        }
    }

    pub(super) fn consumed_steps(&self) -> u64 {
        self.steps
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
            budget.check_time().map_err(|error| {
                ExecutionFault::new(
                    AnalysisDiagnosticCode::UnhandledRuntimeError,
                    span,
                    format!(
                        "interpreter execution exhausted the run-owned wall-time budget: {}",
                        error.message
                    ),
                )
            })?;
        }
        self.steps = self.steps.saturating_add(1);
        Ok(())
    }
}

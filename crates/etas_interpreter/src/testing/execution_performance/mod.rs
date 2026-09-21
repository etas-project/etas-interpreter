use super::{
    FakeHost,
    allocation::{Allocations, measure},
    checked_project,
};
use crate::{
    api::{RunOptions, RunOutcome},
    eval::{EvalContext, EvalContextInput},
    plan::InterpreterPlan,
    value::InterpValue,
};
use std::time::{Duration, Instant};

mod iteration;
mod scope;

struct PreparedExecution {
    checked: etas_frontend::CheckedProject,
    plan: InterpreterPlan,
    runtime: tokio::runtime::Runtime,
}

impl PreparedExecution {
    fn new(source: &str) -> Self {
        let checked = checked_project(source);
        let planned = crate::Interpreter.plan(&checked, crate::api::PlanOptions);
        assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(tokio::task::yield_now());
        Self {
            checked,
            plan: planned.plan.unwrap(),
            runtime,
        }
    }

    // Plan, inputs, runtime, host and evaluator preparation are outside the
    // measured region. Drive the real machine, including cooperative yields.
    fn run(&self, args: &[InterpValue]) -> (InterpValue, Allocations, Duration) {
        let host = FakeHost::new(Default::default());
        let options = RunOptions::default();
        let execution = etas_host::execution::ExecutionScope::new_owned();
        let mut eval = EvalContext::new(EvalContextInput {
            checked: &self.checked,
            plan: &self.plan,
            storage_limits: options.storage_limits,
            event_observer: None,
            execution: execution.clone(),
            host_context: options.host_context,
            model_policy: options.model_policy,
            execution_limits: options.execution_limits,
            consumed_steps: 0,
            current_session: None,
            entry_item: self.checked.entry.unwrap(),
            entry_args: args,
        });
        let ((outcome, elapsed), cost) = measure(|| {
            let start = Instant::now();
            let signal = eval.execute_entry_signal(self.checked.entry.unwrap());
            let outcome = self
                .runtime
                .block_on(crate::driver::execute_entry(&mut eval, signal, &host));
            (outcome.into_outcome(), start.elapsed())
        });
        assert!(eval.diagnostics.is_empty(), "{:?}", eval.diagnostics);
        assert!(eval.checkpoints.is_empty());
        execution.finish_body(false).unwrap();
        let RunOutcome::Completed(value) = outcome else {
            panic!("execution did not complete: {outcome:?}")
        };
        (value, cost, elapsed)
    }
}

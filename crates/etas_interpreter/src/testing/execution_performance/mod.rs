use super::{
    FakeHost,
    allocation::{Allocations, measure},
    checked_project, checked_project_with_options,
};
use crate::{
    api::{RunOptions, RunOutcome},
    eval::{EvalContext, EvalContextInput},
    plan::InterpreterPlan,
    value::InterpValue,
};
use std::time::{Duration, Instant};

mod batch_storage_semijoin;
mod crypto;
mod fuse_projection_scans;
mod fuse_serialization_boundary;
mod iteration;
mod lower_owned_snapshot_cow;
mod lower_resource_region;
mod quality_aware_context_policy;
mod request_preparation;
mod schedule_bounded_pipeline;
mod scope;
mod segmented_concat;
mod suffix_demand_history;
mod synthesize_scoped_index;
mod text_reassignment;
mod versioned_preparation_cse;

struct PreparedExecution {
    checked: etas_frontend::CheckedProject,
    plan: InterpreterPlan,
    runtime: tokio::runtime::Runtime,
}

impl PreparedExecution {
    fn new(source: &str) -> Self {
        let checked = checked_project(source);
        Self::from_checked(checked)
    }

    fn with_optimization(source: &str, optimize_hir: bool) -> Self {
        let checked = checked_project_with_options(
            source,
            etas_frontend::ProjectCompileOptions {
                optimize_hir,
                ..Default::default()
            },
        );
        Self::from_checked(checked)
    }

    fn from_checked(checked: etas_frontend::CheckedProject) -> Self {
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
        let (value, allocations, elapsed, _) = self.run_measured(args);
        (value, allocations, elapsed)
    }

    fn run_measured(&self, args: &[InterpValue]) -> (InterpValue, Allocations, Duration, u64) {
        let host = FakeHost::new(Default::default());
        let options = RunOptions::default();
        let execution = etas_host::execution::ExecutionScope::new_owned();
        let (program, mut run_state) = EvalContextInput {
            replay_keys: crate::eval::replay::RunReplayKeys::start(
                &crate::api::RunOptions::default(),
            )
            .unwrap(),
            code_definitions: options.code_definitions.clone(),
            syntax_limits: options.syntax_limits,
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
        }
        .into_parts();
        let mut eval = EvalContext::new(program, &mut run_state);
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
        let consumed_steps = eval.consumed_steps();
        (value, cost, elapsed, consumed_steps)
    }
}

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

    fn with_optimization_level(
        source: &str,
        optimization_level: etas_frontend::OptimizationLevel,
    ) -> Self {
        Self::with_compile_options(
            source,
            etas_frontend::ProjectCompileOptions {
                optimization_level,
                ..Default::default()
            },
        )
    }

    fn with_quality_context_token_budget(
        source: &str,
        optimization_level: etas_frontend::OptimizationLevel,
        budget: Option<usize>,
    ) -> Self {
        Self::with_compile_options(
            source,
            etas_frontend::ProjectCompileOptions {
                optimization_level,
                quality_context_token_budget: budget,
                ..Default::default()
            },
        )
    }

    fn with_compile_options(source: &str, options: etas_frontend::ProjectCompileOptions) -> Self {
        let checked = checked_project_with_options(source, options);
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

#[derive(Clone, Copy, Debug)]
struct ExecutionMetrics {
    allocations: Allocations,
    elapsed: Duration,
    steps: u64,
}

fn compare_optimization_levels(
    pass_name: &str,
    name: &str,
    source: &str,
    optimized_level: etas_frontend::OptimizationLevel,
) -> (ExecutionMetrics, ExecutionMetrics) {
    use etas_frontend::OptimizationLevel::{O0, O1, O2, O3};

    assert_ne!(optimized_level, O0, "optimized comparison level");
    let levels = [O0, O1, O2, O3];
    let mut measurements = Vec::with_capacity(levels.len());
    for level in levels {
        let prepared = PreparedExecution::with_optimization_level(source, level);
        assert_eq!(prepared.checked.compile_options.optimization_level, level);
        let (value, allocations, elapsed, steps) = prepared.run_measured(&[]);
        measurements.push((
            level,
            value,
            ExecutionMetrics {
                allocations,
                elapsed,
                steps,
            },
        ));
    }

    let baseline_value = &measurements[0].1;
    for (level, value, metrics) in &measurements[1..] {
        assert_eq!(
            value, baseline_value,
            "{name}: semantic result differs at {level}"
        );
        eprintln!(
            "{name} {level}: steps {}; allocations {}; elapsed {:?}",
            metrics.steps, metrics.allocations.count, metrics.elapsed
        );
    }

    let optimized_index = usize::from(optimized_level.number());
    let baseline = measurements[0].2;
    let optimized = measurements[optimized_index].2;
    eprintln!(
        "ETAS_OPT_METRIC,{pass_name},{name},O0,{optimized_level},{},{},{},{},{},{}",
        baseline.steps,
        optimized.steps,
        baseline.allocations.count,
        optimized.allocations.count,
        baseline.elapsed.as_nanos(),
        optimized.elapsed.as_nanos(),
    );
    (baseline, optimized)
}

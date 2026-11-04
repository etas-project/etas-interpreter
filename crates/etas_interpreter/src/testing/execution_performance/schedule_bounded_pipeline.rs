use super::PreparedExecution;

const PROGRAMS: [(&str, &str); 10] = [
    (
        "tool_calls",
        include_str!("../../../fixtures/optimization/schedule_bounded_pipeline/tool_calls.es"),
    ),
    (
        "model_batches",
        include_str!("../../../fixtures/optimization/schedule_bounded_pipeline/model_batches.es"),
    ),
    (
        "file_jobs",
        include_str!("../../../fixtures/optimization/schedule_bounded_pipeline/file_jobs.es"),
    ),
    (
        "mcp_requests",
        include_str!("../../../fixtures/optimization/schedule_bounded_pipeline/mcp_requests.es"),
    ),
    (
        "trace_events",
        include_str!("../../../fixtures/optimization/schedule_bounded_pipeline/trace_events.es"),
    ),
    (
        "retrieval_docs",
        include_str!("../../../fixtures/optimization/schedule_bounded_pipeline/retrieval_docs.es"),
    ),
    (
        "memory_writes",
        include_str!("../../../fixtures/optimization/schedule_bounded_pipeline/memory_writes.es"),
    ),
    (
        "policy_checks",
        include_str!("../../../fixtures/optimization/schedule_bounded_pipeline/policy_checks.es"),
    ),
    (
        "plugin_tasks",
        include_str!("../../../fixtures/optimization/schedule_bounded_pipeline/plugin_tasks.es"),
    ),
    (
        "checkpoint_jobs",
        include_str!("../../../fixtures/optimization/schedule_bounded_pipeline/checkpoint_jobs.es"),
    ),
];

#[test]
fn bounded_pipeline_scheduling_reduces_real_traversal_work() {
    for (name, source) in PROGRAMS {
        let baseline = PreparedExecution::with_optimization(source, false);
        let optimized = PreparedExecution::with_optimization(source, true);
        let (baseline_value, baseline_allocations, baseline_elapsed, baseline_steps) =
            baseline.run_measured(&[]);
        let (optimized_value, optimized_allocations, optimized_elapsed, optimized_steps) =
            optimized.run_measured(&[]);
        assert_eq!(baseline_value, optimized_value, "{name} semantic result");
        assert!(
            optimized_steps < baseline_steps,
            "{name}: steps {baseline_steps} -> {optimized_steps}"
        );
        assert!(
            optimized_allocations.count < baseline_allocations.count,
            "{name}: allocations {} -> {}",
            baseline_allocations.count,
            optimized_allocations.count
        );
        eprintln!(
            "{name}: steps {baseline_steps} -> {optimized_steps}; allocations {} -> {}; elapsed {:?} -> {:?}",
            baseline_allocations.count,
            optimized_allocations.count,
            baseline_elapsed,
            optimized_elapsed
        );
    }
}

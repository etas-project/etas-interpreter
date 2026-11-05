use super::compare_optimization_levels;
use etas_frontend::OptimizationLevel;

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
        let (baseline, optimized) = compare_optimization_levels(
            "ScheduleBoundedPipeline",
            name,
            source,
            OptimizationLevel::O3,
        );
        assert!(
            optimized.steps < baseline.steps,
            "{name}: steps {} -> {}",
            baseline.steps,
            optimized.steps
        );
        assert!(
            optimized.allocations.count < baseline.allocations.count,
            "{name}: allocations {} -> {}",
            baseline.allocations.count,
            optimized.allocations.count
        );
        eprintln!(
            "{name} O0 -> O3: steps {} -> {}; allocations {} -> {}; elapsed {:?} -> {:?}",
            baseline.steps,
            optimized.steps,
            baseline.allocations.count,
            optimized.allocations.count,
            baseline.elapsed,
            optimized.elapsed
        );
    }
}

use super::PreparedExecution;

const PROGRAMS: [(&str, &str); 10] = [
    (
        "tool_catalog",
        include_str!("../../../fixtures/optimization/fuse_projection_scans/tool_catalog.es"),
    ),
    (
        "event_projection",
        include_str!("../../../fixtures/optimization/fuse_projection_scans/event_projection.es"),
    ),
    (
        "memory_entries",
        include_str!("../../../fixtures/optimization/fuse_projection_scans/memory_entries.es"),
    ),
    (
        "mcp_inventory",
        include_str!("../../../fixtures/optimization/fuse_projection_scans/mcp_inventory.es"),
    ),
    (
        "model_capabilities",
        include_str!("../../../fixtures/optimization/fuse_projection_scans/model_capabilities.es"),
    ),
    (
        "workspace_files",
        include_str!("../../../fixtures/optimization/fuse_projection_scans/workspace_files.es"),
    ),
    (
        "policy_clauses",
        include_str!("../../../fixtures/optimization/fuse_projection_scans/policy_clauses.es"),
    ),
    (
        "trace_events",
        include_str!("../../../fixtures/optimization/fuse_projection_scans/trace_events.es"),
    ),
    (
        "repo_paths",
        include_str!("../../../fixtures/optimization/fuse_projection_scans/repo_paths.es"),
    ),
    (
        "context_messages",
        include_str!("../../../fixtures/optimization/fuse_projection_scans/context_messages.es"),
    ),
];

#[test]
fn projection_scan_fusion_reduces_real_iteration_work() {
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

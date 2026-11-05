use super::compare_optimization_levels;
use etas_frontend::OptimizationLevel;

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
        let (baseline, optimized) =
            compare_optimization_levels("FuseProjectionScans", name, source, OptimizationLevel::O2);
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
            "{name} O0 -> O2: steps {} -> {}; allocations {} -> {}; elapsed {:?} -> {:?}",
            baseline.steps,
            optimized.steps,
            baseline.allocations.count,
            optimized.allocations.count,
            baseline.elapsed,
            optimized.elapsed
        );
    }
}

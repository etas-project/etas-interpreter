use super::compare_optimization_levels;
use etas_frontend::OptimizationLevel;

const PROGRAMS: [(&str, &str); 10] = [
    (
        "tool_permissions",
        include_str!("../../../fixtures/optimization/batch_storage_semijoin/tool_permissions.es"),
    ),
    (
        "memory_records",
        include_str!("../../../fixtures/optimization/batch_storage_semijoin/memory_records.es"),
    ),
    (
        "checkpoints",
        include_str!("../../../fixtures/optimization/batch_storage_semijoin/checkpoints.es"),
    ),
    (
        "model_routes",
        include_str!("../../../fixtures/optimization/batch_storage_semijoin/model_routes.es"),
    ),
    (
        "mcp_servers",
        include_str!("../../../fixtures/optimization/batch_storage_semijoin/mcp_servers.es"),
    ),
    (
        "policy_rules",
        include_str!("../../../fixtures/optimization/batch_storage_semijoin/policy_rules.es"),
    ),
    (
        "workspace_files",
        include_str!("../../../fixtures/optimization/batch_storage_semijoin/workspace_files.es"),
    ),
    (
        "provider_keys",
        include_str!("../../../fixtures/optimization/batch_storage_semijoin/provider_keys.es"),
    ),
    (
        "trace_spans",
        include_str!("../../../fixtures/optimization/batch_storage_semijoin/trace_spans.es"),
    ),
    (
        "plugin_states",
        include_str!("../../../fixtures/optimization/batch_storage_semijoin/plugin_states.es"),
    ),
];

#[test]
fn batched_semijoin_reduces_real_storage_scan_work() {
    for (name, source) in PROGRAMS {
        let (baseline, optimized) = compare_optimization_levels(
            "BatchStorageSemijoin",
            name,
            source,
            OptimizationLevel::O2,
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

use super::compare_optimization_levels;
use etas_frontend::OptimizationLevel;

const PROGRAMS: [(&str, &str); 10] = [
    (
        "model_config",
        include_str!("../../../fixtures/optimization/lower_owned_snapshot_cow/model_config.es"),
    ),
    (
        "tool_registry",
        include_str!("../../../fixtures/optimization/lower_owned_snapshot_cow/tool_registry.es"),
    ),
    (
        "session_state",
        include_str!("../../../fixtures/optimization/lower_owned_snapshot_cow/session_state.es"),
    ),
    (
        "policy_set",
        include_str!("../../../fixtures/optimization/lower_owned_snapshot_cow/policy_set.es"),
    ),
    (
        "mcp_catalog",
        include_str!("../../../fixtures/optimization/lower_owned_snapshot_cow/mcp_catalog.es"),
    ),
    (
        "workspace_view",
        include_str!("../../../fixtures/optimization/lower_owned_snapshot_cow/workspace_view.es"),
    ),
    (
        "provider_options",
        include_str!("../../../fixtures/optimization/lower_owned_snapshot_cow/provider_options.es"),
    ),
    (
        "memory_index",
        include_str!("../../../fixtures/optimization/lower_owned_snapshot_cow/memory_index.es"),
    ),
    (
        "checkpoint_state",
        include_str!("../../../fixtures/optimization/lower_owned_snapshot_cow/checkpoint_state.es"),
    ),
    (
        "plugin_manifest",
        include_str!("../../../fixtures/optimization/lower_owned_snapshot_cow/plugin_manifest.es"),
    ),
];

#[test]
fn cow_lowering_reduces_real_owned_snapshot_work() {
    for (name, source) in PROGRAMS {
        let (baseline, optimized) = compare_optimization_levels(
            "LowerOwnedSnapshotCow",
            name,
            source,
            OptimizationLevel::O1,
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
            "{name} O0 -> O1: steps {} -> {}; allocations {} -> {}; elapsed {:?} -> {:?}",
            baseline.steps,
            optimized.steps,
            baseline.allocations.count,
            optimized.allocations.count,
            baseline.elapsed,
            optimized.elapsed
        );
    }
}

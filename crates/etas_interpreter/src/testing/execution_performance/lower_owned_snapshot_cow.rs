use super::PreparedExecution;

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

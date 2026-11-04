use super::PreparedExecution;

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

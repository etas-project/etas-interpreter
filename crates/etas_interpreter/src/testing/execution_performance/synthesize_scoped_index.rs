use super::PreparedExecution;

const PROGRAMS: [(&str, &str); 10] = [
    (
        "mcp_id",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/mcp_id.es"),
    ),
    (
        "pending_interrupt",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/pending_interrupt.es"),
    ),
    (
        "shadow_scope",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/shadow_scope.es"),
    ),
    (
        "tool_name",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/tool_name.es"),
    ),
    (
        "model_capability",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/model_capability.es"),
    ),
    (
        "workspace_file",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/workspace_file.es"),
    ),
    (
        "resource_version",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/resource_version.es"),
    ),
    (
        "memory_entry",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/memory_entry.es"),
    ),
    (
        "provider_route",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/provider_route.es"),
    ),
    (
        "policy_rule",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/policy_rule.es"),
    ),
];

#[test]
fn scoped_index_synthesis_reduces_real_lookup_work() {
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

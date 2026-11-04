use super::PreparedExecution;

const PROGRAMS: [(&str, &str); 10] = [
    (
        "latest_cwd",
        include_str!("../../../fixtures/optimization/suffix_demand_history/latest_cwd.es"),
    ),
    (
        "latest_tool_result",
        include_str!("../../../fixtures/optimization/suffix_demand_history/latest_tool_result.es"),
    ),
    (
        "last_user_message",
        include_str!("../../../fixtures/optimization/suffix_demand_history/last_user_message.es"),
    ),
    (
        "newest_model_config",
        include_str!("../../../fixtures/optimization/suffix_demand_history/newest_model_config.es"),
    ),
    (
        "recent_checkpoint",
        include_str!("../../../fixtures/optimization/suffix_demand_history/recent_checkpoint.es"),
    ),
    (
        "last_retry_error",
        include_str!("../../../fixtures/optimization/suffix_demand_history/last_retry_error.es"),
    ),
    (
        "latest_workspace_root",
        include_str!(
            "../../../fixtures/optimization/suffix_demand_history/latest_workspace_root.es"
        ),
    ),
    (
        "last_policy_decision",
        include_str!(
            "../../../fixtures/optimization/suffix_demand_history/last_policy_decision.es"
        ),
    ),
    (
        "newest_memory_cursor",
        include_str!(
            "../../../fixtures/optimization/suffix_demand_history/newest_memory_cursor.es"
        ),
    ),
    (
        "latest_provider_snapshot",
        include_str!(
            "../../../fixtures/optimization/suffix_demand_history/latest_provider_snapshot.es"
        ),
    ),
];

#[test]
fn suffix_demand_reduces_real_history_scan_work() {
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
            optimized_allocations.count,
        );
        eprintln!(
            "{name}: steps {baseline_steps} -> {optimized_steps}; allocations {} -> {}; elapsed {:?} -> {:?}",
            baseline_allocations.count,
            optimized_allocations.count,
            baseline_elapsed,
            optimized_elapsed,
        );
    }
}

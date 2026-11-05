use super::compare_optimization_levels;
use etas_frontend::OptimizationLevel;

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
        let (baseline, optimized) =
            compare_optimization_levels("SuffixDemandHistory", name, source, OptimizationLevel::O1);
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
            optimized.allocations.count,
        );
        eprintln!(
            "{name} O0 -> O1: steps {} -> {}; allocations {} -> {}; elapsed {:?} -> {:?}",
            baseline.steps,
            optimized.steps,
            baseline.allocations.count,
            optimized.allocations.count,
            baseline.elapsed,
            optimized.elapsed,
        );
    }
}

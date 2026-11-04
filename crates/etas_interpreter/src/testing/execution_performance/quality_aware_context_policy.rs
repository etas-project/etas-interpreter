use super::PreparedExecution;

const PROGRAMS: [(&str, &str); 10] = [
    (
        "repo_map",
        include_str!("../../../fixtures/optimization/quality_aware_context_policy/repo_map.es"),
    ),
    (
        "chat_history",
        include_str!("../../../fixtures/optimization/quality_aware_context_policy/chat_history.es"),
    ),
    (
        "retrieval_pack",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy/retrieval_pack.es"
        ),
    ),
    (
        "session_summary",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy/session_summary.es"
        ),
    ),
    (
        "tool_catalog",
        include_str!("../../../fixtures/optimization/quality_aware_context_policy/tool_catalog.es"),
    ),
    (
        "diagnostic_trace",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy/diagnostic_trace.es"
        ),
    ),
    (
        "code_window",
        include_str!("../../../fixtures/optimization/quality_aware_context_policy/code_window.es"),
    ),
    (
        "memory_recall",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy/memory_recall.es"
        ),
    ),
    (
        "plan_history",
        include_str!("../../../fixtures/optimization/quality_aware_context_policy/plan_history.es"),
    ),
    (
        "policy_summary",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy/policy_summary.es"
        ),
    ),
];

#[test]
fn context_policy_pruning_reduces_real_candidate_work() {
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

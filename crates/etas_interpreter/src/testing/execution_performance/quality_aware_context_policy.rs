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

const BUDGETED_PROGRAMS: [(&str, &str); 10] = [
    (
        "repo_map",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy_budget/repo_map.es"
        ),
    ),
    (
        "chat_history",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy_budget/chat_history.es"
        ),
    ),
    (
        "retrieval_pack",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy_budget/retrieval_pack.es"
        ),
    ),
    (
        "session_summary",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy_budget/session_summary.es"
        ),
    ),
    (
        "tool_catalog",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy_budget/tool_catalog.es"
        ),
    ),
    (
        "diagnostic_trace",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy_budget/diagnostic_trace.es"
        ),
    ),
    (
        "code_window",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy_budget/code_window.es"
        ),
    ),
    (
        "memory_recall",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy_budget/memory_recall.es"
        ),
    ),
    (
        "plan_history",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy_budget/plan_history.es"
        ),
    ),
    (
        "policy_summary",
        include_str!(
            "../../../fixtures/optimization/quality_aware_context_policy_budget/policy_summary.es"
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

#[test]
fn explicit_context_budget_reduces_real_model_input_units() {
    for (name, source) in BUDGETED_PROGRAMS {
        let baseline = PreparedExecution::with_quality_context_token_budget(source, None);
        let optimized = PreparedExecution::with_quality_context_token_budget(source, Some(8));
        let (baseline_value, _, baseline_elapsed, _) = baseline.run_measured(&[]);
        let (optimized_value, _, optimized_elapsed, _) = optimized.run_measured(&[]);
        let crate::value::InterpValue::String(baseline_text) = baseline_value else {
            panic!("{name}: baseline context must be text")
        };
        let crate::value::InterpValue::String(optimized_text) = optimized_value else {
            panic!("{name}: optimized context must be text")
        };
        let baseline_text = baseline_text.into_string();
        let optimized_text = optimized_text.into_string();
        let baseline_units = baseline_text.split_whitespace().count();
        let optimized_units = optimized_text.split_whitespace().count();

        assert_eq!(optimized_units, 8, "{name}: budgeted units");
        assert!(
            optimized_units < baseline_units,
            "{name}: input units {baseline_units} -> {optimized_units}"
        );
        assert!(
            baseline_text.ends_with(&optimized_text),
            "{name}: policy must retain the recent suffix"
        );
        eprintln!(
            "{name}: model input units {baseline_units} -> {optimized_units}; elapsed {:?} -> {:?}",
            baseline_elapsed, optimized_elapsed
        );
    }
}

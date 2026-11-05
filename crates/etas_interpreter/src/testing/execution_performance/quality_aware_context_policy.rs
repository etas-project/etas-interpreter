use super::{PreparedExecution, compare_optimization_levels};
use etas_frontend::OptimizationLevel;

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
        let (baseline, optimized) = compare_optimization_levels(
            "PruneUnusedQualityContext",
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

#[test]
fn explicit_context_budget_reduces_real_model_input_units() {
    for (name, source) in BUDGETED_PROGRAMS {
        for level in [
            OptimizationLevel::O0,
            OptimizationLevel::O1,
            OptimizationLevel::O2,
            OptimizationLevel::O3,
        ] {
            let baseline =
                PreparedExecution::with_quality_context_token_budget(source, level, None);
            let optimized =
                PreparedExecution::with_quality_context_token_budget(source, level, Some(8));
            let (baseline_value, _, baseline_elapsed, _) = baseline.run_measured(&[]);
            let (optimized_value, _, optimized_elapsed, _) = optimized.run_measured(&[]);
            let crate::value::InterpValue::String(baseline_text) = baseline_value else {
                panic!("{name} {level}: baseline context must be text")
            };
            let crate::value::InterpValue::String(optimized_text) = optimized_value else {
                panic!("{name} {level}: optimized context must be text")
            };
            let baseline_text = baseline_text.into_string();
            let optimized_text = optimized_text.into_string();
            let baseline_units = baseline_text.split_whitespace().count();
            let optimized_units = optimized_text.split_whitespace().count();

            assert_eq!(optimized_units, 8, "{name} {level}: budgeted units");
            assert!(
                optimized_units < baseline_units,
                "{name} {level}: input units {baseline_units} -> {optimized_units}"
            );
            assert!(
                baseline_text.ends_with(&optimized_text),
                "{name} {level}: policy must retain the recent suffix"
            );
            eprintln!(
                "{name} {level} policy: model input units {baseline_units} -> {optimized_units}; elapsed {:?} -> {:?}",
                baseline_elapsed, optimized_elapsed
            );
            eprintln!(
                "ETAS_POLICY_METRIC,ApplyQualityContextBudgetPolicy,{name},{level},{baseline_units},{optimized_units},{},{}",
                baseline_elapsed.as_nanos(),
                optimized_elapsed.as_nanos(),
            );
        }
    }
}

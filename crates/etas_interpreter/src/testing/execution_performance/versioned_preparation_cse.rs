use super::PreparedExecution;

const PROGRAMS: [(&str, &str); 10] = [
    (
        "context_delta_cache",
        include_str!(
            "../../../fixtures/optimization/versioned_preparation_cse/context_delta_cache.es"
        ),
    ),
    (
        "repo_map_cache",
        include_str!("../../../fixtures/optimization/versioned_preparation_cse/repo_map_cache.es"),
    ),
    (
        "model_snapshot_cache",
        include_str!(
            "../../../fixtures/optimization/versioned_preparation_cse/model_snapshot_cache.es"
        ),
    ),
    (
        "tool_schema_cache",
        include_str!(
            "../../../fixtures/optimization/versioned_preparation_cse/tool_schema_cache.es"
        ),
    ),
    (
        "provider_capabilities_cache",
        include_str!(
            "../../../fixtures/optimization/versioned_preparation_cse/provider_capabilities_cache.es"
        ),
    ),
    (
        "response_schema_cache",
        include_str!(
            "../../../fixtures/optimization/versioned_preparation_cse/response_schema_cache.es"
        ),
    ),
    (
        "prompt_prefix_cache",
        include_str!(
            "../../../fixtures/optimization/versioned_preparation_cse/prompt_prefix_cache.es"
        ),
    ),
    (
        "route_plan_cache",
        include_str!(
            "../../../fixtures/optimization/versioned_preparation_cse/route_plan_cache.es"
        ),
    ),
    (
        "policy_projection_cache",
        include_str!(
            "../../../fixtures/optimization/versioned_preparation_cse/policy_projection_cache.es"
        ),
    ),
    (
        "serializer_config_cache",
        include_str!(
            "../../../fixtures/optimization/versioned_preparation_cse/serializer_config_cache.es"
        ),
    ),
];

#[test]
fn versioned_preparation_cse_reduces_real_execution_work() {
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

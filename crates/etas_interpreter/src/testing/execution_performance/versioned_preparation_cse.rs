use super::compare_optimization_levels;
use etas_frontend::OptimizationLevel;

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
        let (baseline, optimized) = compare_optimization_levels(
            "VersionedPreparationCse",
            name,
            source,
            OptimizationLevel::O2,
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
            optimized.allocations.count,
        );
        eprintln!(
            "{name} O0 -> O2: steps {} -> {}; allocations {} -> {}; elapsed {:?} -> {:?}",
            baseline.steps,
            optimized.steps,
            baseline.allocations.count,
            optimized.allocations.count,
            baseline.elapsed,
            optimized.elapsed,
        );
    }
}

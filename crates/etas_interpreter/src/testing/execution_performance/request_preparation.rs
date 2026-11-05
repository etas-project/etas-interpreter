use super::compare_optimization_levels;
use etas_frontend::OptimizationLevel;

const PROGRAMS: [(&str, &str); 10] = [
    (
        "tool_schema_catalog",
        include_str!(
            "../../../fixtures/optimization/prepare_versioned_request/tool_schema_catalog.es"
        ),
    ),
    (
        "model_tool_mapping",
        include_str!(
            "../../../fixtures/optimization/prepare_versioned_request/model_tool_mapping.es"
        ),
    ),
    (
        "retry_payload_normalization",
        include_str!(
            "../../../fixtures/optimization/prepare_versioned_request/retry_payload_normalization.es"
        ),
    ),
    (
        "prompt_static_segment",
        include_str!(
            "../../../fixtures/optimization/prepare_versioned_request/prompt_static_segment.es"
        ),
    ),
    (
        "provider_header_plan",
        include_str!(
            "../../../fixtures/optimization/prepare_versioned_request/provider_header_plan.es"
        ),
    ),
    (
        "policy_projection",
        include_str!(
            "../../../fixtures/optimization/prepare_versioned_request/policy_projection.es"
        ),
    ),
    (
        "model_capability_map",
        include_str!(
            "../../../fixtures/optimization/prepare_versioned_request/model_capability_map.es"
        ),
    ),
    (
        "serializer_schema",
        include_str!(
            "../../../fixtures/optimization/prepare_versioned_request/serializer_schema.es"
        ),
    ),
    (
        "plugin_tool_snapshot",
        include_str!(
            "../../../fixtures/optimization/prepare_versioned_request/plugin_tool_snapshot.es"
        ),
    ),
    (
        "immutable_request_options",
        include_str!(
            "../../../fixtures/optimization/prepare_versioned_request/immutable_request_options.es"
        ),
    ),
];

#[test]
fn request_preparation_hoisting_reduces_real_execution_work() {
    for (name, source) in PROGRAMS {
        let (baseline, optimized) = compare_optimization_levels(
            "PrepareVersionedRequest",
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

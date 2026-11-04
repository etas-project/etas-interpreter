use super::PreparedExecution;

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

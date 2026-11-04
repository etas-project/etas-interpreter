use super::PreparedExecution;

const PROGRAMS: [(&str, &str); 10] = [
    (
        "prompt_json",
        include_str!("../../../fixtures/optimization/fuse_serialization_boundary/prompt_json.es"),
    ),
    (
        "tool_schema",
        include_str!("../../../fixtures/optimization/fuse_serialization_boundary/tool_schema.es"),
    ),
    (
        "model_payload",
        include_str!("../../../fixtures/optimization/fuse_serialization_boundary/model_payload.es"),
    ),
    (
        "trace_event",
        include_str!("../../../fixtures/optimization/fuse_serialization_boundary/trace_event.es"),
    ),
    (
        "storage_record",
        include_str!(
            "../../../fixtures/optimization/fuse_serialization_boundary/storage_record.es"
        ),
    ),
    (
        "checkpoint",
        include_str!("../../../fixtures/optimization/fuse_serialization_boundary/checkpoint.es"),
    ),
    (
        "context_delta",
        include_str!("../../../fixtures/optimization/fuse_serialization_boundary/context_delta.es"),
    ),
    (
        "rpc_envelope",
        include_str!("../../../fixtures/optimization/fuse_serialization_boundary/rpc_envelope.es"),
    ),
    (
        "policy_document",
        include_str!(
            "../../../fixtures/optimization/fuse_serialization_boundary/policy_document.es"
        ),
    ),
    (
        "plugin_manifest",
        include_str!(
            "../../../fixtures/optimization/fuse_serialization_boundary/plugin_manifest.es"
        ),
    ),
];

#[test]
fn serialization_boundary_fusion_reduces_real_codec_work() {
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

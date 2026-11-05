use super::compare_optimization_levels;
use etas_frontend::OptimizationLevel;

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
        let (baseline, optimized) = compare_optimization_levels(
            "FuseSerializationBoundary",
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

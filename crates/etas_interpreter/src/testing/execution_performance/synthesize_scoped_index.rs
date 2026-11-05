use super::compare_optimization_levels;
use etas_frontend::OptimizationLevel;

const PROGRAMS: [(&str, &str); 10] = [
    (
        "mcp_id",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/mcp_id.es"),
    ),
    (
        "pending_interrupt",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/pending_interrupt.es"),
    ),
    (
        "shadow_scope",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/shadow_scope.es"),
    ),
    (
        "tool_name",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/tool_name.es"),
    ),
    (
        "model_capability",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/model_capability.es"),
    ),
    (
        "workspace_file",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/workspace_file.es"),
    ),
    (
        "resource_version",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/resource_version.es"),
    ),
    (
        "memory_entry",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/memory_entry.es"),
    ),
    (
        "provider_route",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/provider_route.es"),
    ),
    (
        "policy_rule",
        include_str!("../../../fixtures/optimization/synthesize_scoped_index/policy_rule.es"),
    ),
];

#[test]
fn scoped_index_synthesis_reduces_real_lookup_work() {
    for (name, source) in PROGRAMS {
        let (baseline, optimized) = compare_optimization_levels(
            "SynthesizeScopedIndex",
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
            optimized.allocations.count
        );
        eprintln!(
            "{name} O0 -> O2: steps {} -> {}; allocations {} -> {}; elapsed {:?} -> {:?}",
            baseline.steps,
            optimized.steps,
            baseline.allocations.count,
            optimized.allocations.count,
            baseline.elapsed,
            optimized.elapsed
        );
    }
}

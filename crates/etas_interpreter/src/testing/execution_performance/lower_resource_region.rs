use super::compare_optimization_levels;
use etas_frontend::OptimizationLevel;

const PROGRAMS: [(&str, &str); 10] = [
    (
        "model_client",
        include_str!("../../../fixtures/optimization/lower_resource_region/model_client.es"),
    ),
    (
        "sql_pool",
        include_str!("../../../fixtures/optimization/lower_resource_region/sql_pool.es"),
    ),
    (
        "http_session",
        include_str!("../../../fixtures/optimization/lower_resource_region/http_session.es"),
    ),
    (
        "vector_store",
        include_str!("../../../fixtures/optimization/lower_resource_region/vector_store.es"),
    ),
    (
        "tool_host",
        include_str!("../../../fixtures/optimization/lower_resource_region/tool_host.es"),
    ),
    (
        "cache_handle",
        include_str!("../../../fixtures/optimization/lower_resource_region/cache_handle.es"),
    ),
    (
        "trace_sink",
        include_str!("../../../fixtures/optimization/lower_resource_region/trace_sink.es"),
    ),
    (
        "mcp_channel",
        include_str!("../../../fixtures/optimization/lower_resource_region/mcp_channel.es"),
    ),
    (
        "checkpoint_store",
        include_str!("../../../fixtures/optimization/lower_resource_region/checkpoint_store.es"),
    ),
    (
        "browser_context",
        include_str!("../../../fixtures/optimization/lower_resource_region/browser_context.es"),
    ),
];

#[test]
fn resource_region_lowering_reduces_real_acquisition_work() {
    for (name, source) in PROGRAMS {
        let (baseline, optimized) =
            compare_optimization_levels("LowerResourceRegion", name, source, OptimizationLevel::O3);
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
            "{name} O0 -> O3: steps {} -> {}; allocations {} -> {}; elapsed {:?} -> {:?}",
            baseline.steps,
            optimized.steps,
            baseline.allocations.count,
            optimized.allocations.count,
            baseline.elapsed,
            optimized.elapsed
        );
    }
}

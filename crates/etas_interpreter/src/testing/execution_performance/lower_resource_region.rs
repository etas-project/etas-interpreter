use super::PreparedExecution;

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

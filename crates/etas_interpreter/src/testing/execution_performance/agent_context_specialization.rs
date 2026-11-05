use super::{PreparedExecution, compare_optimization_levels};
use etas_frontend::OptimizationLevel;

const PROGRAMS: [(&str, &str); 11] = [
    (
        "chat_triage",
        include_str!("../../../fixtures/optimization/agent_context_specialization/chat_triage.es"),
    ),
    (
        "checkpoint_resume",
        include_str!(
            "../../../fixtures/optimization/agent_context_specialization/checkpoint_resume.es"
        ),
    ),
    (
        "code_review",
        include_str!("../../../fixtures/optimization/agent_context_specialization/code_review.es"),
    ),
    (
        "mcp_dispatch",
        include_str!("../../../fixtures/optimization/agent_context_specialization/mcp_dispatch.es"),
    ),
    (
        "memory_recall",
        include_str!(
            "../../../fixtures/optimization/agent_context_specialization/memory_recall.es"
        ),
    ),
    (
        "model_routing",
        include_str!(
            "../../../fixtures/optimization/agent_context_specialization/model_routing.es"
        ),
    ),
    (
        "policy_gate",
        include_str!("../../../fixtures/optimization/agent_context_specialization/policy_gate.es"),
    ),
    (
        "release_planning",
        include_str!(
            "../../../fixtures/optimization/agent_context_specialization/release_planning.es"
        ),
    ),
    (
        "retrieval_route",
        include_str!(
            "../../../fixtures/optimization/agent_context_specialization/retrieval_route.es"
        ),
    ),
    (
        "trace_diagnosis",
        include_str!(
            "../../../fixtures/optimization/agent_context_specialization/trace_diagnosis.es"
        ),
    ),
    (
        "workspace_search",
        include_str!(
            "../../../fixtures/optimization/agent_context_specialization/workspace_search.es"
        ),
    ),
];

const PASSES: [&str; 5] = [
    "ProjectAgentObservation",
    "PruneSupersededAgentEvents",
    "ShortCircuitAgentThreshold",
    "SpecializeAgentToolSchemas",
    "SpecializeDeterministicDispatch",
];

#[test]
fn agent_context_specialization_reduces_real_execution_work() {
    assert!(PROGRAMS.len() >= 10);
    for (name, source) in PROGRAMS {
        let prepared = PreparedExecution::with_optimization_level(source, OptimizationLevel::O2);
        for pass in PASSES {
            assert_eq!(
                prepared
                    .checked
                    .optimization_report
                    .pass(pass)
                    .unwrap_or_else(|| panic!("{name}: missing {pass}"))
                    .rewritten,
                1,
                "{name}: {pass}"
            );
        }

        let (baseline, optimized) = compare_optimization_levels(
            "AgentContextSpecialization",
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

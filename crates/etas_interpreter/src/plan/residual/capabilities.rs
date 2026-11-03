//! Residual semantics implemented by this interpreter.
//!
//! These describe what the engine enforces, not credentials, grants or
//! configured services. A compilation checked against them still requires
//! interpreter preparation and installation before anything can execute.

use std::collections::BTreeSet;

use etas_code::residual::target::{
    ResidualComponentKind, ResidualEventPhase, ResidualEvidenceKind, ResidualScopeExitKind,
    ResidualSuspensionKind, ResidualTargetCapabilities, ResidualTargetLimits,
};

/// Engine identity participating in plan support and installation evidence.
pub(crate) const RESIDUAL_ENGINE: &str = "etas.interpreter.residual";
/// Bumped whenever event, evidence, exit or persistence semantics change.
pub(crate) const RESIDUAL_ENGINE_SEMANTIC_VERSION: u32 = 3;

/// The interpreter executes trace-alternative components through the shared
/// kernel: every governed action is previewed before dispatch on all source,
/// default, forwarded, std, agent and tool paths; completed outcomes are
/// correlated by attempt, scope and invocation; every terminal path visits
/// scope exit; monitor state is retained across Host awaits and persisted
/// with checkpoints. One serialized machine owns every scope of a run, and
/// the engine has no concurrent child execution in which a scope could be
/// shared, so concurrent children never observe a scope independently.
pub fn interpreter_residual_capabilities() -> ResidualTargetCapabilities {
    ResidualTargetCapabilities {
        engine: RESIDUAL_ENGINE.into(),
        engine_semantic_version: RESIDUAL_ENGINE_SEMANTIC_VERSION,
        plan_versions: BTreeSet::from([(1, 1)]),
        trace_template_versions: BTreeSet::from([(1, 1)]),
        components: BTreeSet::from([ResidualComponentKind::TraceAlternatives]),
        phases: BTreeSet::from([
            ResidualEventPhase::PreDispatchAttempt,
            ResidualEventPhase::CorrelatedOutcome,
        ]),
        evidence: BTreeSet::from([ResidualEvidenceKind::AcceptedPriorOutcome]),
        exits: ResidualScopeExitKind::ALL.into_iter().collect(),
        suspension: ResidualSuspensionKind::ALL.into_iter().collect(),
        limits: ResidualTargetLimits {
            max_obligations_per_plan: 256,
            max_alternatives: 64,
            max_clauses: 256,
            max_patterns: 512,
        },
    }
}

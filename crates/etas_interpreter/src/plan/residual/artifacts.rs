use etas_utils::{ArtifactKey, ArtifactSet, PassResult};

pub(in crate::plan) const VALIDATED_RESIDUAL_PLANS: ArtifactKey =
    ArtifactKey::new("interpreter.plan", "validated_residual_plans");
pub(in crate::plan) const BOUND_RESIDUAL_SITES: ArtifactKey =
    ArtifactKey::new("interpreter.plan", "bound_residual_sites");
pub(in crate::plan) const PREPARED_RESIDUAL_PLANS: ArtifactKey =
    ArtifactKey::new("interpreter.plan", "prepared_residual_plans");
pub(in crate::plan) const SELECTED_CALLABLE: ArtifactKey =
    ArtifactKey::new("interpreter.plan", "selected_callable");
pub(in crate::plan) const VERIFIED_RESIDUAL_COVERAGE: ArtifactKey =
    ArtifactKey::new("interpreter.plan", "verified_residual_coverage");

pub(in crate::plan) fn produced(key: ArtifactKey) -> PassResult {
    PassResult {
        produced: ArtifactSet::one(key),
        ..PassResult::unchanged()
    }
}

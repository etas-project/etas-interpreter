//! Interpreter preparation of checked residual monitor plans.
//!
//! Inputs are frontend-verified neutral plans and their final revision
//! binding. Preparation revalidates them against this engine's implemented
//! semantics, binds every plan to the checked callable whose invocations
//! activate it, and proves that every residual reference of a selected
//! callable and its reachable callees is covered by an installed scope.
//! The output holds immutable factories only; each invocation activates its
//! own independent scope state at execution time.

use std::collections::BTreeMap;
use std::sync::Arc;

use etas_code::{
    CodeFingerprint, RevisionKey,
    contract::CanonicalEffect,
    residual::identity::{ActivationSiteId, SemanticScopeId},
};
use etas_frontend::CheckedResidualPlan;
use etas_hir::HirItemId;

pub(in crate::plan) mod artifacts;
mod bind;
mod capabilities;
mod coverage;
mod installation;
#[cfg(test)]
mod tests;
mod validate;

pub(super) use bind::BindResidualSitesPass;
pub use capabilities::interpreter_residual_capabilities;
pub(super) use coverage::VerifyResidualCoveragePass;
pub(crate) use installation::ResidualInstallationEvidence;
pub(super) use installation::UsePreparedResidualPlansPass;
pub(super) use validate::ValidateResidualPlansPass;

pub(in crate::plan) struct ValidatedResidualPlans {
    program: etas_frontend::CheckedProject,
}

/// Message shared by every preparation path that finds a residual reference
/// without a verified, bound and installed plan.
pub(crate) const UNINSTALLED_RESIDUAL: &str = "callable requires residual checks but no verified residual plan and installation are available";

/// One verified plan bound to the scope and activation all of its
/// obligations share.
#[derive(Debug)]
pub(crate) struct PreparedTracePlan {
    pub(crate) checked: Arc<CheckedResidualPlan>,
    pub(crate) scope: SemanticScopeId,
    pub(crate) activation: ActivationSiteId,
    pub(crate) bound: CodeFingerprint,
}

/// The immutable factory for scopes activated by invocations of one checked
/// callable. It is not a live monitor and holds no invocation state.
#[derive(Debug)]
pub(crate) struct PreparedResidualScope {
    pub(crate) item: HirItemId,
    pub(crate) revision: RevisionKey,
    pub(crate) plans: Vec<PreparedTracePlan>,
    pub(crate) actions:
        std::collections::HashMap<etas_code::descriptor::EffectTargetInfo, Vec<CanonicalEffect>>,
    pub(crate) approval_target: Option<etas_code::descriptor::EffectTargetInfo>,
}

impl PreparedResidualScope {
    pub(crate) fn is_approval(&self, action: &CanonicalEffect) -> bool {
        self.approval_target.as_ref() == Some(&action.target)
    }

    pub(crate) fn accepts_action(&self, action: &CanonicalEffect) -> bool {
        self.actions
            .get(&action.target)
            .is_some_and(|labels| labels.contains(action))
    }

    pub(crate) fn obligation_ids(&self) -> Vec<etas_code::residual::identity::ObligationId> {
        self.plans
            .iter()
            .flat_map(|plan| {
                plan.checked
                    .plan()
                    .obligations()
                    .iter()
                    .filter(|obligation| {
                        obligation.identity().scope == plan.scope
                            && obligation.identity().activation == plan.activation
                    })
                    .map(|obligation| obligation.id())
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

/// Every residual scope factory of one prepared checked program.
#[derive(Debug, Default)]
pub(crate) struct PreparedResidualPlans {
    target: Option<CodeFingerprint>,
    scopes: BTreeMap<HirItemId, Arc<PreparedResidualScope>>,
}

impl PreparedResidualPlans {
    pub(crate) fn scope(&self, item: HirItemId) -> Option<&Arc<PreparedResidualScope>> {
        self.scopes.get(&item)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }

    /// The target fingerprint this preparation validated against.
    pub(crate) fn target(&self) -> Option<CodeFingerprint> {
        self.target
    }

    /// Canonical identity of the complete preparation, for installation
    /// evidence. It covers every bound plan of every prepared scope.
    pub(crate) fn fingerprint(&self) -> CodeFingerprint {
        let mut parts = Vec::new();
        if let Some(target) = self.target {
            parts.push(target.as_bytes().to_vec());
        }
        for (item, scope) in &self.scopes {
            parts.push((item.0 as u64).to_le_bytes().to_vec());
            parts.push(scope.revision.fingerprint().as_bytes().to_vec());
            for plan in &scope.plans {
                parts.push(plan.checked.plan().hash().fingerprint().as_bytes().to_vec());
                parts.push(plan.bound.as_bytes().to_vec());
            }
        }
        CodeFingerprint::of_parts(
            "etas.interpreter.residual.prepared-plans.v1",
            parts.iter().map(Vec::as_slice),
        )
    }
}

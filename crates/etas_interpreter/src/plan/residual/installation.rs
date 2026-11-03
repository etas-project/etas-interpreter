use std::sync::Arc;

use etas_code::CodeFingerprint;
use etas_frontend::{CheckedCallableRoot, CheckedProject};
use etas_utils::{ArtifactSet, Pass, PassContext, PassManager, PassResult};

use super::{PreparedResidualPlans, artifacts::*};
use crate::plan::context::{PlanContext, plan_pass};

/// Native proof of the coverage pass for an exact immutable execution root.
/// It cannot be deserialized or constructed by a compiler service caller.
#[derive(Clone)]
pub(crate) struct ResidualInstallationEvidence {
    program: CheckedProject,
    root: CheckedCallableRoot,
    prepared: Arc<PreparedResidualPlans>,
    fingerprint: CodeFingerprint,
}

impl ResidualInstallationEvidence {
    pub(super) fn after_coverage(
        program: CheckedProject,
        root: CheckedCallableRoot,
        prepared: Arc<PreparedResidualPlans>,
    ) -> Result<Self, String> {
        if !prepared.is_empty()
            && prepared.target() != Some(super::interpreter_residual_capabilities().fingerprint())
        {
            return Err("residual installation was prepared for another engine".into());
        }
        let fingerprint = prepared.fingerprint();
        Ok(Self {
            program,
            root,
            prepared,
            fingerprint,
        })
    }

    pub(crate) fn validate(
        &self,
        program: &CheckedProject,
        root: CheckedCallableRoot,
        prepared: &Arc<PreparedResidualPlans>,
    ) -> Result<(), String> {
        if !Arc::ptr_eq(program, &self.program)
            || self.root != root
            || !Arc::ptr_eq(prepared, &self.prepared)
        {
            return Err(
                "residual installation does not belong to this checked execution plan".into(),
            );
        }
        Ok(())
    }
}

impl std::fmt::Debug for ResidualInstallationEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResidualInstallationEvidence")
            .field("root", &self.root)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

/// Imports a factory prepared by the layout pipeline, not a serialized plan
/// or a descriptor claiming that a monitor was installed.
pub(in crate::plan) struct UsePreparedResidualPlansPass;

impl Pass<PlanContext<'_>> for UsePreparedResidualPlansPass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.use_prepared_residual_plans")
            .produces(ArtifactSet::one(PREPARED_RESIDUAL_PLANS))
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _pass_context: &PassContext<PlanContext<'_>>,
        _manager: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        context.residual_installation = None;
        if context.prepared_residual.is_none() {
            return PassResult::failed("callable selection has no prepared residual factories");
        }
        produced(PREPARED_RESIDUAL_PLANS)
    }
}

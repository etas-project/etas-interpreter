use etas_core::{Diagnostic, Span};
use etas_frontend::{CheckedCallableRoot, CheckedProject};
use etas_utils::{PassDescriptor, PassKind, UnitKey, UnitOrder, UnitProvider, UnitSelector};

use crate::api::EntryPoint;

use super::{
    ActionMediationTable, GlobalTable, HostRequirementTable, IntrinsicDispatchTable, ResourceTable,
    SlotLayoutTable,
};

pub(super) struct PlanContext<'a> {
    pub(super) project: &'a CheckedProject,
    pub(super) requested_root: Option<CheckedCallableRoot>,
    pub(super) diagnostics: Vec<Diagnostic>,
    pub(super) entry: Option<EntryPoint>,
    pub(super) slots: Option<SlotLayoutTable>,
    pub(super) frames: Option<super::frames::FrameLayoutTable>,
    pub(super) arguments: Option<super::arguments::CallArgumentTable>,
    pub(super) closures: Option<super::ClosureLayoutTable>,
    pub(super) named_variants: Option<super::NamedVariantLayoutTable>,
    pub(super) records: Option<super::RecordLayoutTable>,
    pub(super) globals: Option<GlobalTable>,
    pub(super) resources: Option<ResourceTable>,
    pub(super) dispatch: Option<IntrinsicDispatchTable>,
    pub(super) host_requirements: Option<HostRequirementTable>,
    pub(super) action_mediation: Option<ActionMediationTable>,
    /// Final revision binding of the program's residual plans: the ordinary
    /// program's own binding, or the binding of an emitted candidate.
    pub(super) residual_binding: Option<etas_frontend::CheckedResidualBinding>,
    pub(super) validated_residual: Option<super::residual::ValidatedResidualPlans>,
    pub(super) residual: Option<super::residual::PreparedResidualPlans>,
    pub(super) prepared_residual: Option<std::sync::Arc<super::residual::PreparedResidualPlans>>,
    pub(super) residual_installation: Option<super::residual::ResidualInstallationEvidence>,
}

impl<'a> PlanContext<'a> {
    pub(super) fn new(project: &'a CheckedProject) -> Self {
        Self {
            project,
            requested_root: project.entry.map(CheckedCallableRoot::Item),
            diagnostics: Vec::new(),
            entry: None,
            slots: None,
            frames: None,
            arguments: None,
            closures: None,
            named_variants: None,
            records: None,
            globals: None,
            resources: None,
            dispatch: None,
            host_requirements: None,
            action_mediation: None,
            residual_binding: project.residual_plans().binding().cloned(),
            validated_residual: None,
            residual: None,
            prepared_residual: None,
            residual_installation: None,
        }
    }

    pub(super) fn push_missing_fact(&mut self, span: Span, message: &str) {
        self.diagnostics
            .push(crate::diagnostics::missing_checked_fact(span, message));
    }
}

impl UnitProvider for PlanContext<'_> {
    fn units(&self, _selector: &UnitSelector, _order: UnitOrder) -> Vec<UnitKey> {
        Vec::new()
    }
}

pub(super) fn plan_pass(name: &'static str) -> PassDescriptor {
    PassDescriptor::new(name, PassKind::Plan)
}

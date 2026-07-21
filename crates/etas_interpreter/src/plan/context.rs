use etas_core::{Diagnostic, Span};
use etas_frontend::CheckedProject;
use etas_utils::{PassDescriptor, PassKind, UnitKey, UnitOrder, UnitProvider, UnitSelector};

use crate::api::EntryPoint;

use super::{
    ActionMediationTable, GlobalTable, HostRequirementTable, IntrinsicDispatchTable, ResourceTable,
    SlotLayoutTable,
};

pub(super) struct PlanContext<'a> {
    pub(super) project: &'a CheckedProject,
    pub(super) diagnostics: Vec<Diagnostic>,
    pub(super) entry: Option<EntryPoint>,
    pub(super) slots: Option<SlotLayoutTable>,
    pub(super) globals: Option<GlobalTable>,
    pub(super) resources: Option<ResourceTable>,
    pub(super) dispatch: Option<IntrinsicDispatchTable>,
    pub(super) host_requirements: Option<HostRequirementTable>,
    pub(super) action_mediation: Option<ActionMediationTable>,
}

impl<'a> PlanContext<'a> {
    pub(super) fn new(project: &'a CheckedProject) -> Self {
        Self {
            project,
            diagnostics: Vec::new(),
            entry: None,
            slots: None,
            globals: None,
            resources: None,
            dispatch: None,
            host_requirements: None,
            action_mediation: None,
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

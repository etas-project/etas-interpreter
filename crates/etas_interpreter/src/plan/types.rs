use std::sync::Arc;

use etas_core::Diagnostic;

use crate::api::EntryPoint;

use super::{
    ActionMediationTable, GlobalTable, HostRequirementTable, IntrinsicDispatchTable, ResourceTable,
    SlotLayoutTable,
};

#[derive(Clone, Debug)]
pub struct InterpreterPlan {
    pub(super) prepared: Arc<PreparedProgram>,
    pub(super) program: etas_frontend::CheckedProject,
    pub root: etas_frontend::CheckedCallableRoot,
    pub(crate) known_std_types: crate::eval::KnownStdTypes,
    pub(super) spec_facts: Arc<std::sync::OnceLock<etas_types::SpecFacts>>,
    pub entry: EntryPoint,
    pub slots: Arc<SlotLayoutTable>,
    pub(crate) frames: Arc<super::frames::FrameLayoutTable>,
    pub(crate) arguments: Arc<super::arguments::CallArgumentTable>,
    pub closures: Arc<super::ClosureLayoutTable>,
    pub named_variants: Arc<super::NamedVariantLayoutTable>,
    pub records: Arc<super::RecordLayoutTable>,
    pub globals: Arc<GlobalTable>,
    pub resources: Arc<ResourceTable>,
    pub dispatch: IntrinsicDispatchTable,
    pub host_requirements: HostRequirementTable,
    pub action_mediation: ActionMediationTable,
    pub diagnostics: Vec<Diagnostic>,
}

/// Checked program-wide data has no executable root or invocation authority.
#[derive(Debug)]
pub(crate) struct PreparedProgram {
    pub(super) program: etas_frontend::CheckedProject,
    pub(super) known_std_types: crate::eval::KnownStdTypes,
    pub(super) spec_facts: Arc<std::sync::OnceLock<etas_types::SpecFacts>>,
    pub(super) slots: Arc<SlotLayoutTable>,
    pub(super) frames: Arc<super::frames::FrameLayoutTable>,
    pub(super) arguments: Arc<super::arguments::CallArgumentTable>,
    pub(super) closures: Arc<super::ClosureLayoutTable>,
    pub(super) named_variants: Arc<super::NamedVariantLayoutTable>,
    pub(super) records: Arc<super::RecordLayoutTable>,
    pub(super) globals: Arc<GlobalTable>,
    pub(super) resources: Arc<ResourceTable>,
    pub(super) dispatch: IntrinsicDispatchTable,
    pub(super) diagnostics: Vec<Diagnostic>,
}

use std::sync::Arc;

use etas_core::Diagnostic;

use crate::api::EntryPoint;

use super::{
    ActionMediationTable, GlobalTable, HostRequirementTable, IntrinsicDispatchTable, ResourceTable,
    SlotLayoutTable,
};

#[derive(Clone, Debug)]
pub struct InterpreterPlan {
    pub entry: EntryPoint,
    pub slots: Arc<SlotLayoutTable>,
    pub(crate) frames: super::frames::FrameLayoutTable,
    pub(crate) arguments: super::arguments::CallArgumentTable,
    pub closures: super::ClosureLayoutTable,
    pub named_variants: super::NamedVariantLayoutTable,
    pub records: super::RecordLayoutTable,
    pub globals: GlobalTable,
    pub resources: ResourceTable,
    pub dispatch: IntrinsicDispatchTable,
    pub host_requirements: HostRequirementTable,
    pub action_mediation: ActionMediationTable,
    pub diagnostics: Vec<Diagnostic>,
}

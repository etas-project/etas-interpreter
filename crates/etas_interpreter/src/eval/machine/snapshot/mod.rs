mod call_site;
mod call_target;
#[cfg(test)]
mod capture_alias_tests;
mod capture_context;
mod capture_identity;
mod continuation;
mod frame;
mod frame_dependencies;
#[cfg(test)]
mod frame_graph_tests;
mod machine_frame;
mod message;
#[cfg(test)]
mod message_tests;
mod model;
pub(crate) mod ordering;
pub(crate) mod program;
mod validator;
mod value;
mod value_capture;
pub(crate) mod value_compare;
mod value_membership;
mod value_restore;

pub(crate) use capture_context::CaptureContext;
pub(crate) use frame::RestoreContext;
pub(in crate::eval::machine) use frame::RestoredResidualState;
pub(in crate::eval::machine) use frame::SuspensionRestoreTable;
pub(crate) use model::{capture_model_policy, restore_model_policy};
pub(crate) use validator::SnapshotValidator;

pub(crate) struct SnapshotRestoreResources<'a> {
    pub locked_model: Option<etas_host::ModelName>,
    pub limits: &'a etas_host::StorageLimits,
    pub revisions: Option<std::sync::Arc<crate::api::code::PreparedRevisionSet>>,
    pub root: Option<CheckpointRoot>,
    pub ordering: Option<ordering::OrderedRestoreChecks>,
    /// Freshly prepared residual plans of the root program, required whenever
    /// the checkpoint carries live residual state.
    pub residual: Option<ResidualRestoreInput>,
}

pub(crate) struct ResidualRestoreInput {
    pub prepared: std::sync::Arc<crate::plan::residual::PreparedResidualPlans>,
    pub log: std::rc::Rc<crate::control::residual::ResidualRunLog>,
    pub limits: crate::control::residual::ResidualStateLimits,
}

#[derive(Clone)]
pub(crate) struct CheckpointRoot {
    pub program: crate::value::CheckedExecutionProgram,
    pub args: Vec<crate::value::InterpValue>,
}

pub(crate) struct CapturedMachineSnapshot {
    pub root: Option<CheckpointRoot>,
    pub args: Vec<crate::orchestration::ValueSnapshot>,
    pub machine: crate::orchestration::MachineSnapshot,
    pub revisions: std::collections::BTreeMap<
        etas_code::RevisionKey,
        std::sync::Arc<crate::api::code::PreparedCodeRevision>,
    >,
}

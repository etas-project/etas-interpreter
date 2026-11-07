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
mod input;
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

pub(crate) use capture_context::{CaptureContext, SharedFrameCapture};
pub(in crate::eval::machine) use frame::RestoredResidualState;
pub(in crate::eval::machine) use frame::SuspensionRestoreTable;
pub(crate) use frame::{RestoreContext, SharedFrameRestore};
pub(crate) use model::{capture_model_policy, restore_model_policy};
pub(crate) use validator::SnapshotValidator;

pub(crate) struct SnapshotRestoreResources<'a> {
    pub task: Option<TaskSnapshotRestoreResources<'a>>,
    pub locked_model: Option<etas_host::ModelName>,
    pub limits: &'a etas_host::StorageLimits,
    pub revisions: Option<std::sync::Arc<crate::api::code::PreparedRevisionSet>>,
    pub root: Option<CheckpointRoot>,
    pub ordering: Option<ordering::OrderedRestoreChecks>,
    /// Freshly prepared residual plans of the root program, required whenever
    /// the checkpoint carries live residual state.
    pub residual: Option<ResidualRestoreInput>,
}

pub(crate) struct TaskSnapshotRestoreResources<'a> {
    pub inherited_handlers: usize,
    pub scheduler: &'a crate::driver::scheduler::TaskSchedulerMetadataSnapshot,
    pub identity: crate::driver::scheduler::TaskIdentity,
    pub frames: SharedFrameRestore,
}

pub(crate) struct RestoredMachineSnapshot {
    pub machine: super::EvalMachine,
    pub contexts: std::collections::HashMap<usize, RestoreContext>,
}

impl RestoredMachineSnapshot {
    pub(crate) fn restore_auxiliary(
        &mut self,
        snapshot: &crate::orchestration::MachineSnapshot,
        index: u32,
        source: &crate::value::CheckedExecutionProgram,
        revisions: Option<&crate::api::code::PreparedRevisionSet>,
    ) -> Result<crate::control::Continuation, String> {
        let entry = snapshot
            .auxiliary
            .get(index as usize)
            .ok_or("task continuation references a missing machine auxiliary entry")?;
        let program = crate::value::CheckedExecutionProgram::restore_identity(
            &entry.program,
            source,
            revisions,
        )?;
        let key = std::sync::Arc::as_ptr(&program.checked) as usize;
        if !self.contexts.contains_key(&key) {
            let context = self.active_context().related_program(program.clone());
            self.contexts.insert(key, context);
        }
        let context = self
            .contexts
            .get_mut(&key)
            .expect("auxiliary program context was inserted");
        context.execution_program = Some(program);
        entry.continuation.clone().restore_with(context)
    }

    pub(crate) fn active_context(&mut self) -> &mut RestoreContext {
        let program = self
            .machine
            .program_scope
            .as_ref()
            .map(|scope| &scope.program)
            .or_else(|| {
                self.machine
                    .checkpoint_root
                    .as_ref()
                    .map(|root| &root.program)
            })
            .expect("restored task machine has a checked root");
        self.contexts
            .get_mut(&(std::sync::Arc::as_ptr(&program.checked) as usize))
            .expect("active task program has a restore context")
    }
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

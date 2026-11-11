use std::sync::Arc;

use etas_frontend::CheckedProgram;

use crate::api::{
    InterpreterPlan,
    code::{PreparedCodeDefinitions, PreparedCodeRevision},
};

/// Immutable code ownership only. Invocation authority and mutable execution
/// state belong to EvalRunState, not to a callable's defining program.
#[derive(Clone, Debug)]
pub(crate) struct CheckedExecutionProgram {
    pub checked: CheckedProgram,
    pub plan: Arc<InterpreterPlan>,
    pub definitions: Option<Arc<PreparedCodeDefinitions>>,
    pub revision: Option<Arc<PreparedCodeRevision>>,
}

impl CheckedExecutionProgram {
    pub(crate) fn restore_identity(
        saved: &crate::orchestration::ExecutionProgramSnapshot,
        source: &Self,
        revisions: Option<&crate::api::code::PreparedRevisionSet>,
    ) -> Result<Self, String> {
        use crate::orchestration::{CheckpointProgramReference, ExecutionRootSnapshot as R};
        let owns_program = match (&saved.program, &source.revision) {
            (CheckpointProgramReference::Source, None) => true,
            (CheckpointProgramReference::Revision { revision: saved }, Some(current)) => {
                **saved == crate::orchestration::CheckpointCodeRevision::for_revision(current)
            }
            _ => false,
        };
        if owns_program {
            let root = match saved.root {
                R::Item(item) => etas_frontend::CheckedCallableRoot::Item(item),
                R::Lambda(expr) => etas_frontend::CheckedCallableRoot::Lambda(expr),
                R::StdReference(expr) => etas_frontend::CheckedCallableRoot::StdReference(expr),
            };
            let plan = source
                .plan
                .select_callable(&source.checked, root)
                .map_err(|errors| {
                    format!("checkpoint callable has no rechecked plan: {errors:?}")
                })?;
            return Ok(Self {
                checked: source.checked.clone(),
                plan: Arc::new(plan),
                definitions: source.definitions.clone(),
                revision: source.revision.clone(),
            });
        }
        revisions
            .ok_or("checkpoint program has no prepared revision set")?
            .execution_snapshot(saved)
    }

    pub(crate) fn checkpoint_identity(
        &self,
        revisions: &mut std::collections::BTreeMap<
            etas_code::RevisionKey,
            Arc<PreparedCodeRevision>,
        >,
    ) -> crate::orchestration::ExecutionProgramSnapshot {
        use crate::orchestration::{
            CheckpointCodeRevision, CheckpointProgramReference, ExecutionProgramSnapshot,
            ExecutionRootSnapshot,
        };
        let program = match &self.revision {
            Some(revision) => {
                revisions.insert(revision.revision().key(), revision.clone());
                CheckpointProgramReference::Revision {
                    revision: Box::new(CheckpointCodeRevision::for_revision(revision)),
                }
            }
            None => CheckpointProgramReference::Source,
        };
        let root = match self.plan.root {
            etas_frontend::CheckedCallableRoot::Item(item) => ExecutionRootSnapshot::Item(item),
            etas_frontend::CheckedCallableRoot::Lambda(expr) => ExecutionRootSnapshot::Lambda(expr),
            etas_frontend::CheckedCallableRoot::StdReference(expr) => {
                ExecutionRootSnapshot::StdReference(expr)
            }
        };
        ExecutionProgramSnapshot { program, root }
    }
}

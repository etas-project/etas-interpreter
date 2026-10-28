use std::{collections::BTreeMap, sync::Arc};

use etas_code::RevisionKey;
use etas_frontend::CheckedProgram;

use super::PreparedAgentRevision;
use crate::{
    orchestration::{CheckpointAgentReference, CheckpointCodeRevision},
    value::AgentRevisionValue,
};

/// Native revisions compiled under the current locked source universe. Saved
/// identities can select these objects, but can never create executable code.
#[derive(Debug)]
pub struct PreparedRevisionSet {
    source: CheckedProgram,
    revisions: BTreeMap<RevisionKey, Arc<PreparedAgentRevision>>,
}

struct PreparedRevisionProgram<'a> {
    program: &'a Arc<crate::plan::PreparedProgram>,
    definitions: Arc<super::PreparedCodeDefinitions>,
    revision: Option<Arc<PreparedAgentRevision>>,
}

impl PreparedRevisionProgram<'_> {
    fn execution(
        self,
        plan: Arc<crate::plan::InterpreterPlan>,
    ) -> crate::value::CheckedExecutionProgram {
        crate::value::CheckedExecutionProgram {
            checked: self.program.checked().clone(),
            plan,
            definitions: Some(self.definitions),
            revision: self.revision,
        }
    }
}

impl PreparedRevisionSet {
    pub fn new(source: CheckedProgram) -> Self {
        Self {
            source,
            revisions: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, revision: Arc<PreparedAgentRevision>) -> Result<(), String> {
        if !Arc::ptr_eq(&self.source, revision.type_bridge().original_program()) {
            return Err("restored revision belongs to a different checked source universe".into());
        }
        let identity = CheckpointCodeRevision::for_revision(&revision);
        identity.validate()?;
        if let Some(previous) = self.revisions.get(&revision.revision().key()) {
            if CheckpointCodeRevision::for_revision(previous) != identity {
                return Err("restored revision key has conflicting checked identities".into());
            }
            if !Arc::ptr_eq(previous.program(), revision.program()) {
                return Err("restored revision key has ambiguous native program owners".into());
            }
            return Ok(());
        }
        self.revisions.insert(revision.revision().key(), revision);
        Ok(())
    }

    pub(crate) fn checked_revision(
        &self,
        identity: &CheckpointCodeRevision,
    ) -> Result<&Arc<PreparedAgentRevision>, String> {
        identity.validate()?;
        let revision = self
            .revisions
            .get(&identity.revision_key)
            .ok_or("checkpoint callable has no freshly prepared revision")?;
        if CheckpointCodeRevision::for_revision(revision) != *identity {
            return Err(
                "checkpoint callable identity differs from its freshly prepared revision".into(),
            );
        }
        revision.type_bridge().bind_runtime_owner(revision);
        Ok(revision)
    }

    pub(crate) fn checked_program(
        &self,
        reference: &crate::orchestration::CheckpointProgramReference,
    ) -> Result<CheckedProgram, String> {
        use crate::orchestration::CheckpointProgramReference;
        Ok(match reference {
            CheckpointProgramReference::Source => self.source.clone(),
            CheckpointProgramReference::Revision { revision } => {
                self.checked_revision(revision)?.program().clone()
            }
        })
    }

    fn prepared_program(
        &self,
        reference: &crate::orchestration::CheckpointProgramReference,
    ) -> Result<PreparedRevisionProgram<'_>, String> {
        use crate::orchestration::CheckpointProgramReference;
        Ok(match reference {
            CheckpointProgramReference::Source => {
                let owner = self
                    .revisions
                    .values()
                    .next()
                    .ok_or("checkpoint source callable has no checked revision environment")?;
                PreparedRevisionProgram {
                    program: owner.source_program(),
                    definitions: owner.source_definitions.clone(),
                    revision: None,
                }
            }
            CheckpointProgramReference::Revision { revision } => {
                let owner = self.checked_revision(revision)?;
                PreparedRevisionProgram {
                    program: owner.plan().prepared_program(),
                    definitions: owner.definitions.clone(),
                    revision: Some(owner.clone()),
                }
            }
        })
    }

    pub(crate) fn execution_callable(
        &self,
        metadata: &crate::orchestration::ScopedCallableMetadata,
        target: &crate::orchestration::CallTargetSnapshot,
    ) -> Result<crate::value::CheckedExecutionProgram, String> {
        let prepared = self.prepared_program(&metadata.native_program)?;
        let plan = prepared
            .program
            .select_snapshot_callable(metadata.native_type, target)?;
        Ok(prepared.execution(plan))
    }

    pub(crate) fn execution_snapshot(
        &self,
        snapshot: &crate::orchestration::ExecutionProgramSnapshot,
    ) -> Result<crate::value::CheckedExecutionProgram, String> {
        use crate::orchestration::ExecutionRootSnapshot as R;
        let root = match snapshot.root {
            R::Item(item) => etas_frontend::CheckedCallableRoot::Item(item),
            R::Lambda(expr) => etas_frontend::CheckedCallableRoot::Lambda(expr),
            R::StdReference(expr) => etas_frontend::CheckedCallableRoot::StdReference(expr),
        };
        let prepared = self.prepared_program(&snapshot.program)?;
        let plan = Arc::new(prepared.program.select_callable(root).map_err(|errors| {
            format!("checkpoint execution root has no current checked plan: {errors:?}")
        })?);
        Ok(prepared.execution(plan))
    }

    pub(crate) fn callable_projections(
        &self,
        metadata: &crate::orchestration::ScopedCallableMetadata,
        presented: &CheckedProgram,
    ) -> Result<Vec<Arc<crate::plan::code::CallableValueProjection>>, String> {
        use crate::plan::code::{CallableValueProjection, RevisionProjection};
        let (mut program, mut ty) = (
            self.checked_program(&metadata.native_program)?,
            metadata.native_type,
        );
        if !matches!(
            program.type_store.get(ty),
            Some(etas_types::Type::Function(_))
        ) {
            return Err("checkpoint native callable has no checked function type".into());
        }
        let mut result = Vec::with_capacity(metadata.projections.len());
        for saved in &metadata.projections {
            let revision = self.checked_revision(&saved.revision)?;
            let edge = if saved.to_candidate {
                RevisionProjection::Candidate(revision.type_bridge().clone())
            } else {
                RevisionProjection::Original(revision.type_bridge().clone())
            };
            if !Arc::ptr_eq(&program, edge.source()) || saved.from != ty {
                return Err(
                    "checkpoint callable projection chain has a different source interface".into(),
                );
            }
            let projection = CallableValueProjection::restore_checked(edge, saved.from, saved.to)?;
            (program, ty) = (projection.target().0.clone(), projection.target().1);
            result.push(Arc::new(projection));
        }
        if !Arc::ptr_eq(&program, presented) {
            return Err("checkpoint callable is presented in a different checked program".into());
        }
        Ok(result)
    }

    pub(crate) fn restore(
        &self,
        reference: &CheckpointAgentReference,
        program: &CheckedProgram,
    ) -> Result<AgentRevisionValue, String> {
        reference.revision.validate()?;
        let revision = self
            .revisions
            .get(&reference.revision.revision_key)
            .ok_or("checkpoint agent reference has no freshly prepared revision")?;
        if CheckpointCodeRevision::for_revision(revision) != reference.revision {
            return Err(
                "checkpoint agent reference differs from its freshly prepared revision".into(),
            );
        }
        let source_bridge = if Arc::ptr_eq(program, &self.source) {
            None
        } else {
            Some(
                self.revisions
                    .values()
                    .find(|candidate| Arc::ptr_eq(candidate.program(), program))
                    .ok_or("checkpoint agent reference has no checked caller revision")?
                    .type_bridge()
                    .clone(),
            )
        };
        AgentRevisionValue::restore_checkpoint_reference(
            reference,
            revision.clone(),
            source_bridge,
            program,
        )
    }
}

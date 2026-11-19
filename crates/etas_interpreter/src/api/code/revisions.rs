use std::{collections::BTreeMap, sync::Arc};

use etas_code::RevisionKey;
use etas_frontend::CheckedProgram;

use super::PreparedCodeRevision;
use crate::{
    orchestration::{CheckpointAgentReference, CheckpointCodeRevision},
    value::AgentRevisionValue,
};

/// Native revisions compiled under the current locked source universe. Saved
/// identities can select these objects, but can never create executable code.
#[derive(Debug)]
pub struct PreparedRevisionSet {
    source: CheckedProgram,
    revisions: BTreeMap<RevisionKey, Arc<PreparedCodeRevision>>,
    source_prepared: std::sync::OnceLock<Result<Arc<crate::plan::PreparedProgram>, String>>,
    source_definitions: Option<Arc<super::PreparedCodeDefinitions>>,
}

struct PreparedRevisionProgram<'a> {
    program: &'a Arc<crate::plan::PreparedProgram>,
    definitions: Option<Arc<super::PreparedCodeDefinitions>>,
    revision: Option<Arc<PreparedCodeRevision>>,
}

impl PreparedRevisionProgram<'_> {
    fn execution(
        self,
        plan: Arc<crate::plan::InterpreterPlan>,
    ) -> crate::value::CheckedExecutionProgram {
        crate::value::CheckedExecutionProgram {
            checked: self.program.checked().clone(),
            plan,
            definitions: self.definitions,
            revision: self.revision,
            foreign: None,
        }
    }
}

impl PreparedRevisionSet {
    pub fn new(source: CheckedProgram) -> Self {
        Self {
            source,
            revisions: BTreeMap::new(),
            source_prepared: Default::default(),
            source_definitions: None,
        }
    }

    pub(crate) fn with_source_definitions(
        mut self,
        definitions: Option<Arc<super::PreparedCodeDefinitions>>,
    ) -> Self {
        self.source_definitions = definitions;
        self
    }

    pub fn insert(&mut self, revision: Arc<PreparedCodeRevision>) -> Result<(), String> {
        if !Arc::ptr_eq(&self.source, revision.type_bridge().original_program())
            && !revision.saved_external_type_bridge().is_some_and(|bridge| {
                Arc::ptr_eq(&self.source, bridge.original_program())
                    && Arc::ptr_eq(revision.program(), bridge.candidate_program())
            })
        {
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
    ) -> Result<&Arc<PreparedCodeRevision>, String> {
        identity.validate()?;
        let revision = self.revisions.get(&identity.revision_key).ok_or(
            "checkpoint callable requires freshly prepared revisions; its revision is missing",
        )?;
        if CheckpointCodeRevision::for_revision(revision) != *identity {
            return Err(
                "checkpoint callable identity differs from its freshly prepared revision".into(),
            );
        }
        revision.type_bridge().bind_runtime_owner(revision);
        if let Some(bridge) = revision.saved_external_type_bridge() {
            bridge.bind_runtime_owner(revision);
        }
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
            CheckpointProgramReference::StandardLibrary { owner } => self
                .checked_program(owner)?
                .standard_library
                .as_ref()
                .ok_or("checkpoint requires the selected source std bundle")?
                .program
                .clone(),
        })
    }

    fn reference_for(
        &self,
        program: &CheckedProgram,
    ) -> Result<crate::orchestration::CheckpointProgramReference, String> {
        use crate::orchestration::CheckpointProgramReference as P;
        let candidates = std::iter::once((self.source.clone(), P::Source)).chain(
            self.revisions.values().map(|revision| {
                (
                    revision.program().clone(),
                    P::Revision {
                        revision: Box::new(CheckpointCodeRevision::for_revision(revision)),
                    },
                )
            }),
        );
        for (checked, identity) in candidates {
            if Arc::ptr_eq(&checked, program) {
                return Ok(identity);
            }
            if checked
                .standard_library
                .as_ref()
                .is_some_and(|library| Arc::ptr_eq(&library.program, program))
            {
                return Ok(P::StandardLibrary {
                    owner: Box::new(identity),
                });
            }
        }
        Err("checkpoint program is outside the current source/revision/std universe".into())
    }

    pub(crate) fn project_type(
        &self,
        source: &crate::orchestration::CheckpointProgramReference,
        target: &CheckedProgram,
        ty: etas_types::TypeId,
    ) -> Result<etas_types::TypeId, String> {
        let program = self.checked_program(source)?;
        if Arc::ptr_eq(&program, target) {
            return Ok(ty);
        }
        let ty = self.type_to_source(source, ty)?;
        self.type_from_source(&self.reference_for(target)?, ty)
    }

    fn type_to_source(
        &self,
        reference: &crate::orchestration::CheckpointProgramReference,
        ty: etas_types::TypeId,
    ) -> Result<etas_types::TypeId, String> {
        use crate::orchestration::CheckpointProgramReference as P;
        match reference {
            P::Source => Ok(ty),
            P::Revision { revision } => {
                let revision = self.checked_revision(revision)?;
                revision
                    .type_bridge()
                    .runtime_to_original(revision.program(), ty)
                    .map_err(|error| error.to_string())
            }
            P::StandardLibrary { owner } => {
                let parent = self.prepared_program(owner)?;
                let library = parent
                    .program
                    .standard_library()
                    .ok_or("checkpoint type has no checked std bridge")?;
                let ty = library
                    .bridge()
                    .runtime_to_original(library.prepared().checked(), ty)
                    .map_err(|error| error.to_string())?;
                self.type_to_source(owner, ty)
            }
        }
    }

    fn type_from_source(
        &self,
        reference: &crate::orchestration::CheckpointProgramReference,
        ty: etas_types::TypeId,
    ) -> Result<etas_types::TypeId, String> {
        use crate::orchestration::CheckpointProgramReference as P;
        match reference {
            P::Source => Ok(ty),
            P::Revision { revision } => {
                let revision = self.checked_revision(revision)?;
                revision
                    .type_bridge()
                    .runtime_to_candidate(&self.source, ty)
                    .map_err(|error| error.to_string())
            }
            P::StandardLibrary { owner } => {
                let ty = self.type_from_source(owner, ty)?;
                let parent = self.prepared_program(owner)?;
                let library = parent
                    .program
                    .standard_library()
                    .ok_or("checkpoint type has no checked std bridge")?;
                library
                    .bridge()
                    .runtime_to_candidate(parent.program.checked(), ty)
                    .map_err(|error| error.to_string())
            }
        }
    }

    fn prepared_program(
        &self,
        reference: &crate::orchestration::CheckpointProgramReference,
    ) -> Result<PreparedRevisionProgram<'_>, String> {
        use crate::orchestration::CheckpointProgramReference;
        Ok(match reference {
            CheckpointProgramReference::Source => {
                if let Some(owner) = self.revisions.values().next() {
                    return Ok(PreparedRevisionProgram {
                        program: owner.source_program(),
                        definitions: Some(owner.source_definitions.clone()),
                        revision: None,
                    });
                }
                let prepared = self.source_prepared.get_or_init(|| {
                    crate::plan::prepare_program(&self.source).map_err(|errors| {
                        format!("checkpoint source preparation failed: {errors:?}")
                    })
                });
                PreparedRevisionProgram {
                    program: prepared.as_ref().map_err(Clone::clone)?,
                    definitions: self.source_definitions.clone(),
                    revision: None,
                }
            }
            CheckpointProgramReference::Revision { revision } => {
                let owner = self.checked_revision(revision)?;
                PreparedRevisionProgram {
                    program: owner.plan().prepared_program(),
                    definitions: Some(owner.definitions.clone()),
                    revision: Some(owner.clone()),
                }
            }
            CheckpointProgramReference::StandardLibrary { owner } => {
                let parent = self.prepared_program(owner)?;
                let library = parent
                    .program
                    .standard_library()
                    .ok_or("checkpoint parent has no checked std execution owner")?;
                PreparedRevisionProgram {
                    program: library.prepared(),
                    definitions: None,
                    revision: None,
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
        let prepared = self.prepared_program(&snapshot.program)?;
        let root = match &snapshot.root {
            R::Item(item) => etas_frontend::CheckedCallableRoot::Item(*item),
            R::Lambda(expr) => etas_frontend::CheckedCallableRoot::Lambda(*expr),
            R::StdReference(expr) => etas_frontend::CheckedCallableRoot::StdReference(*expr),
            R::ParameterDefault(reference) => etas_frontend::CheckedCallableRoot::ParameterDefault(
                prepared
                    .program
                    .parameter_default(reference)
                    .ok_or("checkpoint default root has no matching expression revision")?
                    .parameter,
            ),
        };
        let plan = Arc::new(prepared.program.select_callable(root).map_err(|errors| {
            format!("checkpoint execution root has no current checked plan: {errors:?}")
        })?);
        Ok(prepared.execution(plan))
    }

    pub(crate) fn checked_projection(
        &self,
        saved: &crate::orchestration::CheckedProjectionSnapshot,
    ) -> Result<crate::plan::code::RevisionProjection, String> {
        use crate::plan::code::RevisionProjection;
        let bridge = match &saved.origin {
            crate::orchestration::CallableProjectionOrigin::Revision { revision, external } => {
                let revision = self.checked_revision(revision)?;
                if *external {
                    revision
                        .saved_external_type_bridge()
                        .ok_or("checkpoint requires missing external type bridge")?
                } else {
                    revision.type_bridge()
                }
            }
            crate::orchestration::CallableProjectionOrigin::StandardLibrary { owner } => self
                .prepared_program(owner)?
                .program
                .standard_library()
                .ok_or("checkpoint projection has no current checked std owner")?
                .bridge(),
        };
        Ok(if saved.to_candidate {
            RevisionProjection::Candidate(bridge.clone())
        } else {
            RevisionProjection::Original(bridge.clone())
        })
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
            let bridge = match &saved.origin {
                crate::orchestration::CallableProjectionOrigin::Revision { revision, external } => {
                    let revision = self.checked_revision(revision)?;
                    if *external {
                        revision
                            .saved_external_type_bridge()
                            .ok_or("checkpoint requires missing external type bridge")?
                    } else {
                        revision.type_bridge()
                    }
                }
                crate::orchestration::CallableProjectionOrigin::StandardLibrary { owner } => self
                    .prepared_program(owner)?
                    .program
                    .standard_library()
                    .ok_or("checkpoint projection has no current checked std owner")?
                    .bridge(),
            };
            let edge = if saved.to_candidate {
                RevisionProjection::Candidate(bridge.clone())
            } else {
                RevisionProjection::Original(bridge.clone())
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
            .ok_or("checkpoint agent reference requires freshly prepared revisions; its revision is missing")?;
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

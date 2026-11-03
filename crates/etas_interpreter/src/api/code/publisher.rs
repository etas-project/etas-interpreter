use std::sync::Arc;

use etas_code::{CodeFingerprint, DefinitionKey, RevisionInfo};
use etas_core::{Diagnostic, Severity};
use etas_frontend::{
    CheckedProgram,
    code::service::{CandidatePublisher, CheckedCandidate},
};

#[cfg(test)]
use crate::Interpreter;
use crate::api::{EntryPoint, InterpreterPlan};

mod checked_reference;
mod plans;
mod source;
pub use checked_reference::CheckedReferencePublicationError;
pub use source::{RevisionCheckpointError, SavedCodeRevision};

/// Engine-owned immutable preparation. It contains no caller frame, Host
/// context, authority grant, model request or mutable agent binding.
pub struct PreparedCodeRevision {
    candidate: CheckedCandidate,
    plan: Arc<InterpreterPlan>,
    source_entries: plans::RevisionEntryPlans,
    candidate_entries: plans::RevisionEntryPlans,
    type_bridge: Arc<crate::plan::code::RevisionTypeBridge>,
    external_type_bridge: Option<Arc<crate::plan::code::RevisionTypeBridge>>,
    pub(crate) targets: crate::plan::code::RevisionTargets,
    pub(crate) definitions: Arc<super::PreparedCodeDefinitions>,
    pub(crate) source_definitions: Arc<super::PreparedCodeDefinitions>,
    pub(crate) frozen_bindings: etas_frontend::code::definition::FrozenReflectionBindings,
    // Retained executable values remain charged to the issuing compiler store.
    lease: Option<etas_code::artifact::CheckedCodeRef>,
    pub(crate) reflection: Option<PreparedRevisionDefinition>,
}

pub(crate) struct PreparedRevisionDefinition {
    pub definition: etas_code::definition::DefinitionRef,
    pub backend: Arc<etas_frontend::code::definition::NativeDefinitionStore>,
}

impl std::fmt::Debug for PreparedCodeRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedCodeRevision")
            .field("identity", self.identity())
            .field("revision", self.revision())
            .finish_non_exhaustive()
    }
}

impl PreparedCodeRevision {
    pub(crate) fn matches_external_definition(
        &self,
        store: &etas_frontend::code::definition::NativeDefinitionStore,
        definition: &etas_code::definition::DefinitionRef,
    ) -> bool {
        self.candidate
            .matches_external_definition(store, definition)
    }
    pub(crate) fn retain_service_lease(
        &mut self,
        lease: etas_code::artifact::CheckedCodeRef,
    ) -> Result<(), String> {
        if lease.identity() != self.identity()
            || lease.revision() != self.revision()
            || lease.snapshot_fingerprint() != self.snapshot_fingerprint()
        {
            return Err("executable revision cannot retain a mismatched checked-code lease".into());
        }
        self.lease = Some(lease);
        Ok(())
    }

    pub(crate) fn retain_reflected_definition(
        &mut self,
        backend: Arc<etas_frontend::code::definition::NativeDefinitionStore>,
        input: &etas_code::definition::DefinitionRef,
    ) -> Result<(), String> {
        let definition = backend
            .reflect_checked(input, &self.candidate, Default::default())
            .map_err(|error| error.to_string())?;
        let source_backend = if self.candidate.matches_external_definition(&backend, input) {
            self.external_type_bridge = Some(Arc::new(
                crate::plan::code::RevisionTypeBridge::from_external_application(
                    &self.candidate,
                    &backend,
                    input,
                    self.definitions.definitions().catalog(),
                )
                .map_err(|error| error.to_string())?,
            ));
            backend
                .restored_source_store(input, &self.candidate, Default::default())
                .map_err(|error| error.to_string())?
        } else {
            backend.clone()
        };
        self.frozen_bindings = etas_frontend::code::definition::FrozenReflectionBindings::build(
            source_backend.clone(),
            &self.candidate,
            self.definitions.definitions().catalog(),
        )
        .map_err(|error| error.to_string())?;
        self.source_definitions = Arc::new(super::PreparedCodeDefinitions::new(source_backend));
        self.definitions = Arc::new(super::PreparedCodeDefinitions::new(Arc::new(
            etas_frontend::code::definition::NativeDefinitionStore::new(
                self.definitions.definitions().catalog().clone(),
                backend.syntax_operations().clone(),
                Default::default(),
            ),
        )));
        self.reflection = Some(PreparedRevisionDefinition {
            definition,
            backend,
        });
        Ok(())
    }
    pub fn identity(&self) -> &DefinitionKey {
        self.candidate.identity()
    }
    pub(crate) fn external_type_bridge(
        &self,
        caller: &CheckedProgram,
        expected: etas_types::TypeId,
    ) -> Result<Option<&Arc<crate::plan::code::RevisionTypeBridge>>, String> {
        let Some(bridge) = &self.external_type_bridge else {
            return Ok(None);
        };
        let reflected = self
            .reflection
            .as_ref()
            .ok_or("external bridge has no source definition")?;
        let view = reflected
            .backend
            .resolve(&reflected.definition)
            .map_err(|error| error.to_string())?;
        let application = view
            .external_application()
            .ok_or("external bridge has no checked application")?;
        if !Arc::ptr_eq(caller, application.program()) {
            return Err("external bridge belongs to a different checked caller".into());
        }
        let Some(etas_types::Type::Function(flow)) = caller.type_store.get(expected) else {
            return Err("external bridge expected type is not callable".into());
        };
        let signature = application.signature();
        if flow.input != signature.params
            || flow.output != signature.output
            || flow.effects != signature.effects
        {
            return Err("external bridge differs from the checked applied signature".into());
        }
        Ok(Some(bridge))
    }

    pub(crate) fn saved_external_type_bridge(
        &self,
    ) -> Option<&Arc<crate::plan::code::RevisionTypeBridge>> {
        self.external_type_bridge.as_ref()
    }
    pub(crate) fn frozen_environment(
        &self,
    ) -> &etas_frontend::code::definition::FrozenDefinitionEnvironment {
        self.candidate.frozen()
    }
    pub fn revision(&self) -> &RevisionInfo {
        self.candidate.revision()
    }
    pub fn snapshot_fingerprint(&self) -> CodeFingerprint {
        self.candidate.snapshot_fingerprint()
    }
    pub fn program(&self) -> &CheckedProgram {
        self.candidate.program()
    }
    pub fn plan(&self) -> &Arc<InterpreterPlan> {
        &self.plan
    }
    pub(crate) fn source_program(&self) -> &Arc<crate::plan::PreparedProgram> {
        self.source_entries.program()
    }
    pub(crate) fn callable_plan(
        &self,
        item: etas_hir::HirItemId,
        frozen: bool,
    ) -> Result<&Arc<InterpreterPlan>, String> {
        (if frozen {
            &self.source_entries
        } else {
            &self.candidate_entries
        })
        .get(item)
    }
    pub fn type_bridge(&self) -> &Arc<crate::plan::code::RevisionTypeBridge> {
        &self.type_bridge
    }
    pub fn original_definitions(
        &self,
    ) -> &Arc<etas_frontend::code::definition::NativeDefinitionStore> {
        self.frozen_bindings.source()
    }
    pub fn entry(&self) -> EntryPoint {
        EntryPoint {
            item: self.candidate.entry(),
        }
    }
    pub fn create_run<'a>(
        self: &'a Arc<Self>,
        args: Vec<crate::api::InterpValue>,
        host: &'a dyn crate::host::HostServices,
        options: crate::api::RunOptions,
    ) -> crate::api::RunInvocation<'a> {
        self.type_bridge.bind_runtime_owner(self);
        crate::api::RunInvocation::prepared(self, args, host, options)
    }

    pub fn create_resume<'a>(
        self: &'a Arc<Self>,
        checkpoint: &'a crate::api::InterpreterCheckpoint,
        host: &'a dyn crate::host::HostServices,
        options: crate::api::RunOptions,
    ) -> crate::api::RunInvocation<'a> {
        self.type_bridge.bind_runtime_owner(self);
        crate::api::RunInvocation::resume_prepared(self, checkpoint, host, options)
    }
}

#[derive(Debug)]
pub enum CandidatePublicationError {
    PlanRejected {
        diagnostics: Vec<Diagnostic>,
    },
    MissingPlan,
    EntryMismatch,
    UnmaterializedRuntimeSources {
        packages: Vec<etas_frontend::ExternalPackageId>,
    },
    SourceBindings(etas_frontend::code::definition::NativeDefinitionError),
    TypeBridge(crate::plan::code::RevisionTypeBridgeError),
}

impl std::fmt::Display for CandidatePublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlanRejected { diagnostics } => {
                write!(
                    formatter,
                    "candidate plan rejected with {} diagnostics",
                    diagnostics.len()
                )?;
                for diagnostic in diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.severity == Severity::Error)
                    .take(4)
                {
                    write!(formatter, "; {:?}: {}", diagnostic.code, diagnostic.message)?;
                }
                Ok(())
            }
            Self::MissingPlan => {
                formatter.write_str("candidate planning completed without an execution plan")
            }
            Self::EntryMismatch => {
                formatter.write_str("candidate plan selected a different default entry")
            }
            Self::UnmaterializedRuntimeSources { packages } => {
                write!(
                    formatter,
                    "candidate runtime source requirements are not materialized for packages {packages:?}"
                )
            }
            Self::SourceBindings(error) => error.fmt(formatter),
            Self::TypeBridge(error) => error.fmt(formatter),
        }
    }
}
impl std::error::Error for CandidatePublicationError {}

#[derive(Default)]
pub struct InterpreterCandidatePublisher;

impl CandidatePublisher for InterpreterCandidatePublisher {
    type Artifact = Arc<PreparedCodeRevision>;
    type Error = CandidatePublicationError;

    fn publish(&mut self, candidate: CheckedCandidate) -> Result<Self::Artifact, Self::Error> {
        let requirements = &candidate.program().reachability.runtime_source_requirements;
        if !requirements.dependencies.is_empty() {
            return Err(CandidatePublicationError::UnmaterializedRuntimeSources {
                packages: requirements.dependencies.keys().copied().collect(),
            });
        }
        let (original, compiled) =
            candidate
                .revision_catalogs(Default::default())
                .map_err(|error| {
                    CandidatePublicationError::TypeBridge(
                        crate::plan::code::RevisionTypeBridgeError::Projection(error),
                    )
                })?;
        let type_bridge =
            crate::plan::code::RevisionTypeBridge::from_catalogs(&candidate, &original, &compiled)
                .map_err(CandidatePublicationError::TypeBridge)?;
        let targets = crate::plan::code::RevisionTargets::build(&candidate, &original, &compiled)
            .map_err(CandidatePublicationError::TypeBridge)?;
        let syntax = Arc::new(etas_frontend::code::service::NativeCodeSyntaxOperations::default());
        let source = Arc::new(etas_frontend::code::definition::NativeDefinitionStore::new(
            original,
            syntax.clone(),
            Default::default(),
        ));
        let source_definitions = Arc::new(super::PreparedCodeDefinitions::new(source.clone()));
        let frozen_bindings = etas_frontend::code::definition::FrozenReflectionBindings::build(
            source, &candidate, &compiled,
        )
        .map_err(CandidatePublicationError::SourceBindings)?;
        let definitions = Arc::new(super::PreparedCodeDefinitions::new(Arc::new(
            etas_frontend::code::definition::NativeDefinitionStore::new(
                compiled,
                syntax,
                Default::default(),
            ),
        )));
        // The candidate's residual plans execute only through the binding to
        // its frozen-contract revision created when it was emitted.
        let planned = crate::plan::build_plan_with_residual_binding(
            candidate.program(),
            candidate.program().entry,
            candidate.residual_binding(),
        );
        if planned
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
        {
            return Err(CandidatePublicationError::PlanRejected {
                diagnostics: planned.diagnostics,
            });
        }
        let plan = planned.plan.ok_or(CandidatePublicationError::MissingPlan)?;
        if plan.entry.item != candidate.entry() {
            return Err(CandidatePublicationError::EntryMismatch);
        }
        let plan = Arc::new(plan);
        let candidate_entries = plans::RevisionEntryPlans::build(
            candidate.program(),
            plan.clone(),
            targets
                .callables
                .values()
                .map(|target| target.candidate_item),
        )?;
        let source_items = targets
            .frozen_methods
            .values()
            .map(|(_, callable)| callable)
            .chain(targets.frozen_agents.values())
            .map(|callable| {
                targets
                    .frozen_callables
                    .get(callable)
                    .map(|target| target.original_item)
                    .ok_or_else(|| {
                        CandidatePublicationError::TypeBridge(
                            crate::plan::code::RevisionTypeBridgeError::MissingCallable(
                                callable.clone(),
                            ),
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let source_entries = plans::RevisionEntryPlans::prepare_source(
            type_bridge.original_program(),
            source_items.into_iter(),
        )?;
        Ok(Arc::new(PreparedCodeRevision {
            candidate,
            plan,
            source_entries,
            candidate_entries,
            type_bridge: Arc::new(type_bridge),
            external_type_bridge: None,
            targets,
            definitions,
            source_definitions,
            frozen_bindings,
            lease: None,
            reflection: None,
        }))
    }
}

#[cfg(test)]
mod tests;

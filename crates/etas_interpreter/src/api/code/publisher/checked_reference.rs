use std::sync::Arc;

use etas_code::{artifact::CheckedCodeRef, definition::AgentDefinitionRef};
use etas_frontend::code::{
    definition::NativeDefinitionStore,
    service::{CandidatePublisher, CheckedCandidateResolver},
};

use super::{CandidatePublicationError, InterpreterCandidatePublisher, PreparedAgentRevision};

#[derive(Debug)]
pub enum CheckedReferencePublicationError {
    Resolution(etas_frontend::code::service::CandidateResolutionError),
    IdentityMismatch,
    Publication(CandidatePublicationError),
    SharedBeforePublication,
    Definition(String),
    Lease(String),
}

impl std::fmt::Display for CheckedReferencePublicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resolution(error) => error.fmt(f),
            Self::IdentityMismatch => {
                f.write_str("resolved checked candidate disagrees with the compiler lease")
            }
            Self::Publication(error) => error.fmt(f),
            Self::SharedBeforePublication => f.write_str(
                "candidate was shared before its source and service lease were installed",
            ),
            Self::Definition(reason) => write!(f, "candidate source binding failed: {reason}"),
            Self::Lease(reason) => write!(f, "candidate service lease binding failed: {reason}"),
        }
    }
}
impl std::error::Error for CheckedReferencePublicationError {}

impl InterpreterCandidatePublisher {
    /// Resolves a live service-owned lease before preparing execution. A saved
    /// fingerprint or serialized source bundle cannot enter this boundary.
    pub fn publish_checked_reference(
        &mut self,
        resolver: &dyn CheckedCandidateResolver,
        reference: &CheckedCodeRef,
        definitions: Arc<NativeDefinitionStore>,
        definition: &AgentDefinitionRef,
    ) -> Result<Arc<PreparedAgentRevision>, CheckedReferencePublicationError> {
        use CheckedReferencePublicationError as E;
        let candidate = resolver
            .resolve_checked_candidate(reference)
            .map_err(E::Resolution)?;
        if candidate.identity() != reference.identity()
            || candidate.revision() != reference.revision()
            || candidate.snapshot_fingerprint() != reference.snapshot_fingerprint()
        {
            return Err(E::IdentityMismatch);
        }
        let mut prepared = self.publish(candidate).map_err(E::Publication)?;
        let unpublished = Arc::get_mut(&mut prepared).ok_or(E::SharedBeforePublication)?;
        unpublished
            .retain_reflected_definition(definitions, definition)
            .map_err(E::Definition)?;
        unpublished
            .retain_service_lease(reference.clone())
            .map_err(E::Lease)?;
        Ok(prepared)
    }
}

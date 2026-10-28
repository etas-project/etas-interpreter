use etas_code::definition::SourceBundleLimits;

use super::{RevisionCheckpointError, SavedAgentRevision};
use crate::api::InterpreterCheckpoint;

#[derive(Debug)]
pub(crate) struct CheckpointCodePins {
    revisions: std::collections::BTreeMap<
        etas_code::RevisionKey,
        std::sync::Arc<super::PreparedAgentRevision>,
    >,
}

impl CheckpointCodePins {
    pub(crate) fn new(
        revisions: std::collections::BTreeMap<
            etas_code::RevisionKey,
            std::sync::Arc<super::PreparedAgentRevision>,
        >,
    ) -> Self {
        Self { revisions }
    }
}

/// Exports only the source of revisions captured by this live checkpoint.
/// Imported artifacts carry no executable pins; their caller retains the source
/// bundle separately. Source visibility and each bundle's limits still apply.
pub fn checkpoint_source_revisions(
    checkpoint: &InterpreterCheckpoint,
    limits: SourceBundleLimits,
) -> Result<Vec<SavedAgentRevision>, RevisionCheckpointError> {
    checkpoint
        .retained_code
        .as_ref()
        .ok_or(RevisionCheckpointError::MissingRetainedSource)?
        .revisions
        .values()
        .map(|revision| revision.save_source(limits))
        .collect()
}

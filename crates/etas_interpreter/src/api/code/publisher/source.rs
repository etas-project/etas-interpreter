use etas_code::{
    CodeFingerprint,
    definition::{AgentSourceBundle, SourceBundleLimits},
};

use super::PreparedAgentRevision;

/// Source restoration data, not executable evidence. An application must
/// restore the draft and submit it through the normal authorized compiler.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedAgentRevision {
    pub schema: u32,
    pub source: AgentSourceBundle,
    pub snapshot_fingerprint: CodeFingerprint,
}

#[derive(Debug)]
pub enum RevisionCheckpointError {
    UnsupportedSchema(u32),
    CheckpointIdentity(String),
    MissingRetainedSource,
    Source(etas_frontend::code::definition::NativeDefinitionError),
    InvalidSource(etas_code::definition::SourceBundleError),
    RevisionMismatch,
    SnapshotMismatch,
}

impl SavedAgentRevision {
    pub fn validate(&self, limits: SourceBundleLimits) -> Result<(), RevisionCheckpointError> {
        if self.schema != 1 {
            return Err(RevisionCheckpointError::UnsupportedSchema(self.schema));
        }
        self.source
            .validate(limits)
            .map_err(RevisionCheckpointError::InvalidSource)
    }
}

impl PreparedAgentRevision {
    pub fn save_checkpoint_source(
        &self,
        checkpoint: &crate::api::InterpreterCheckpoint,
        limits: SourceBundleLimits,
    ) -> Result<SavedAgentRevision, RevisionCheckpointError> {
        checkpoint
            .compilation
            .validate_for_project(self.program(), checkpoint.entry_item)
            .map_err(RevisionCheckpointError::CheckpointIdentity)?;
        if checkpoint.code_revision.as_ref()
            != Some(&crate::orchestration::CheckpointCodeRevision::for_revision(
                self,
            ))
        {
            return Err(RevisionCheckpointError::RevisionMismatch);
        }
        if checkpoint.entry_item != self.entry().item {
            return Err(RevisionCheckpointError::RevisionMismatch);
        }
        self.save_source(limits)
    }

    pub fn save_source(
        &self,
        limits: SourceBundleLimits,
    ) -> Result<SavedAgentRevision, RevisionCheckpointError> {
        let reflected = self
            .reflection
            .as_ref()
            .ok_or(RevisionCheckpointError::MissingRetainedSource)?;
        let source = reflected
            .backend
            .save_source(&reflected.definition, limits)
            .map_err(RevisionCheckpointError::Source)?;
        if source.target != *self.identity()
            || source.revision_key != self.revision().key()
            || &source.revision != self.revision().inputs()
        {
            return Err(RevisionCheckpointError::RevisionMismatch);
        }
        Ok(SavedAgentRevision {
            schema: 1,
            source,
            snapshot_fingerprint: self.snapshot_fingerprint(),
        })
    }

    /// Called only after normal compilation has produced this native revision.
    /// Stored analysis never substitutes for checking or current authority.
    pub fn validate_restored_source(
        &self,
        saved: &SavedAgentRevision,
        limits: SourceBundleLimits,
    ) -> Result<(), RevisionCheckpointError> {
        saved.validate(limits)?;
        if saved.source.target != *self.identity()
            || saved.source.revision_key != self.revision().key()
            || &saved.source.revision != self.revision().inputs()
        {
            return Err(RevisionCheckpointError::RevisionMismatch);
        }
        if saved.snapshot_fingerprint != self.snapshot_fingerprint() {
            return Err(RevisionCheckpointError::SnapshotMismatch);
        }
        Ok(())
    }
}

impl std::fmt::Display for RevisionCheckpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSchema(schema) => {
                write!(f, "unsupported saved agent revision schema {schema}")
            }
            Self::CheckpointIdentity(reason) => {
                write!(f, "checkpoint compilation identity mismatch: {reason}")
            }
            Self::MissingRetainedSource => {
                f.write_str("checkpoint revision has no accessible retained source")
            }
            Self::Source(error) => error.fmt(f),
            Self::InvalidSource(error) => error.fmt(f),
            Self::RevisionMismatch => f.write_str(
                "saved agent revision differs from the prepared target or source revision",
            ),
            Self::SnapshotMismatch => {
                f.write_str("saved agent checked snapshot differs from the recompiled candidate")
            }
        }
    }
}
impl std::error::Error for RevisionCheckpointError {}

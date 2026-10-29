use super::*;
use crate::api::code::SavedCodeRevision;
use etas_code::definition::SourceBundleLimits;

/// Inert, bounded source restoration input. Native references are decoded only
/// after the current invocation has authorized and recompiled its sources.
pub struct SourceCheckpoint {
    value: super::super::CheckpointDocument,
    compilation: CheckpointCompilationIdentity,
    pub(crate) header: ResumeHeader,
    pub(crate) sources: Vec<SavedCodeRevision>,
}

pub(crate) struct ResumeHeader {
    pub entry: HirItemId,
    pub host: CheckpointHostState,
    pub progress: ExecutionProgressSnapshot,
    pub trace: TraceSnapshot,
    pub session: Option<String>,
}

impl SourceCheckpoint {
    pub fn from_document(
        value: super::super::CheckpointDocument,
        checked: &etas_frontend::CheckedProject,
    ) -> Result<Self, InterpreterCodecError> {
        let schema = required_str(&value, "schema")?;
        if schema != crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA {
            return Err(InterpreterCodecError::new(format!(
                "unsupported checkpoint artifact schema `{schema}`"
            )));
        }
        let checkpoint = required_obj(&value, "checkpoint")?;
        if !required_obj(checkpoint, "code_revision")?.is_null() {
            return Err(InterpreterCodecError::new(
                "source entry restoration requires a checked source root; use prepared revision restoration for a native root",
            ));
        }
        let entry = HirItemId(required_u32(checkpoint, "entry_item")?);
        let compilation = compilation_identity_from_json(required_obj(checkpoint, "compilation")?)?;
        compilation
            .validate_for_project(checked, entry)
            .map_err(InterpreterCodecError::new)?;
        let host = checkpoint_host_state_from_json(required_obj(checkpoint, "host_state")?)?;
        host.budget
            .restore()
            .map_err(|error| InterpreterCodecError::new(error.to_string()))?;
        let trace = required_obj(checkpoint, "trace")?;
        let header = ResumeHeader {
            entry,
            host,
            progress: execution_progress_from_json(required_obj(
                checkpoint,
                "execution_progress",
            )?)?,
            trace: TraceSnapshot {
                events_recorded: required_usize(trace, "events_recorded")?,
                next_message: required_u32(trace, "next_message")?,
                next_host_request: required_u32(trace, "next_host_request")?,
            },
            session: required_optional_string(checkpoint, "current_session")?,
        };
        let raw = required_array(&value, "code_sources")?;
        header
            .progress
            .original_limits
            .validate()
            .map_err(InterpreterCodecError::new)?;
        if header
            .progress
            .original_limits
            .max_steps
            .is_some_and(|limit| header.progress.consumed_steps > limit.get())
        {
            return Err(InterpreterCodecError::new(
                "checkpoint consumed steps exceed its saved execution limit",
            ));
        }
        if u32::try_from(raw.len())
            .ok()
            .and_then(|count| header.trace.next_host_request.checked_add(count))
            .is_none_or(|next| next == u32::MAX)
        {
            return Err(InterpreterCodecError::new(
                "checkpoint has insufficient Host request identity space for source restoration",
            ));
        }
        let mut sources = Vec::with_capacity(raw.len());
        let mut revisions = std::collections::BTreeSet::new();
        for source in raw {
            let source: SavedCodeRevision =
                serde::Deserialize::deserialize(source).map_err(|error| {
                    InterpreterCodecError::new(format!("invalid checkpoint source: {error}"))
                })?;
            source
                .validate(SourceBundleLimits::default())
                .map_err(|error| InterpreterCodecError::new(error.to_string()))?;
            if !revisions.insert(source.source.revision_key) {
                return Err(InterpreterCodecError::new(
                    "checkpoint contains duplicate source revisions",
                ));
            }
            sources.push(source);
        }
        Ok(Self {
            value,
            compilation,
            header,
            sources,
        })
    }

    pub fn entry_item(&self) -> HirItemId {
        self.header.entry
    }

    pub(crate) fn validate_project(
        &self,
        checked: &etas_frontend::CheckedProject,
    ) -> Result<(), String> {
        self.compilation
            .validate_for_project(checked, self.header.entry)
    }

    pub(crate) fn decode(
        &self,
        checked: &etas_frontend::CheckedProject,
        limits: &etas_host::StorageLimits,
        revisions: Option<&std::sync::Arc<crate::api::code::PreparedRevisionSet>>,
    ) -> Result<InterpreterCheckpoint, InterpreterCodecError> {
        checkpoint_from_json_with_revisions(limits, &self.value, checked, revisions)
    }
}

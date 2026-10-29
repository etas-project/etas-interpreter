use super::*;
use crate::orchestration::CheckpointCodeRevision;
use std::sync::Arc;

impl SnapshotValidator<'_> {
    pub(super) fn validate_root_revision(
        &self,
        checkpoint: &InterpreterCheckpoint,
    ) -> Result<(), String> {
        if checkpoint.code_revision.is_some()
            != self.checked.definition_sources.requires_revision_context()
        {
            return Err("checkpoint code revision presence differs from the checked program's revision source".into());
        }
        let Some(identity) = &checkpoint.code_revision else {
            return Ok(());
        };
        identity.validate()?;
        let prepared = match self.execution_revision {
            Some(prepared) => {
                if &CheckpointCodeRevision::for_revision(prepared) != identity {
                    return Err(
                        "checkpoint code identity differs from its prepared execution revision"
                            .into(),
                    );
                }
                prepared
            }
            None => self
                .revisions
                .ok_or("checkpoint code revision requires freshly prepared revisions")?
                .checked_revision(identity)?
                .as_ref(),
        };
        if !Arc::ptr_eq(self.checked, prepared.program())
            || checkpoint.entry_item != prepared.entry().item
        {
            return Err(
                "checkpoint code revision differs from its checked execution program or entry"
                    .into(),
            );
        }
        Ok(())
    }
}

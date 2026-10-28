use std::sync::Arc;

use etas_frontend::CheckedProject;
use etas_hir::HirItemId;

use crate::{
    api::{RunOptions, code::PreparedAgentRevision},
    orchestration::{CheckpointCodeRevision, InterpreterCheckpoint},
};

pub(super) fn bind_options(
    project: &CheckedProject,
    entry: HirItemId,
    prepared: Option<&Arc<PreparedAgentRevision>>,
    options: &mut RunOptions,
) -> Result<(), String> {
    let Some(revision) = prepared else {
        return if project.definition_sources.requires_revision_context() {
            Err("compiled program execution requires its prepared code revision".into())
        } else {
            Ok(())
        };
    };
    if !Arc::ptr_eq(project, revision.program()) || entry != revision.entry().item {
        return Err(
            "prepared revision disagrees with the requested program or default entry".into(),
        );
    }
    if options
        .code_definitions
        .as_ref()
        .is_some_and(|definitions| {
            !Arc::ptr_eq(
                definitions.definitions(),
                revision.definitions.definitions(),
            )
        })
    {
        return Err(
            "prepared revision definitions cannot be replaced by invocation options".into(),
        );
    }
    options.code_definitions = Some(revision.definitions.clone());
    Ok(())
}

pub(super) fn validate_checkpoint(
    project: &CheckedProject,
    checkpoint: &InterpreterCheckpoint,
    prepared: Option<&Arc<PreparedAgentRevision>>,
) -> Result<(), String> {
    match (&checkpoint.code_revision, prepared) {
        (None, None) if !project.definition_sources.requires_revision_context() => Ok(()),
        (Some(saved), Some(revision)) => {
            saved.validate()?;
            if saved != &CheckpointCodeRevision::for_revision(revision) {
                return Err("checkpoint requires its exact freshly prepared code revision".into());
            }
            Ok(())
        }
        _ => Err("checkpoint requires its exact freshly prepared code revision".into()),
    }
}

pub(super) fn restore_arguments(
    project: &CheckedProject,
    checkpoint: &InterpreterCheckpoint,
    revisions: Option<Arc<crate::api::code::PreparedRevisionSet>>,
) -> Result<Vec<crate::value::InterpValue>, String> {
    let mut context =
        crate::eval::machine::snapshot::RestoreContext::for_program(project.clone(), revisions);
    checkpoint
        .args
        .iter()
        .map(|argument| {
            crate::orchestration::ValueSnapshot::capture(argument)?.restore_with(&mut context)
        })
        .collect()
}

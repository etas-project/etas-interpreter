mod prepare;
mod run;
mod shutdown;

pub(crate) use prepare::{resume_checkpoint_inner, run_checked_inner};
pub(crate) use run::drive_invocation;

pub(crate) struct BodyResult {
    pub outcome: crate::api::RunOutcome,
    pub diagnostics: Vec<etas_core::Diagnostic>,
    pub events: Vec<crate::orchestration::WorkflowEvent>,
    pub checkpoints: Vec<crate::orchestration::InterpreterCheckpoint>,
}

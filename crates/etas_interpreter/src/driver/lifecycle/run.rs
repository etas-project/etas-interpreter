use super::{BodyResult, resume_checkpoint_inner, run_checked_inner};
use crate::api::lifecycle::InvocationInput;
use crate::api::{RunInfrastructureError, RunInvocation, RunOutcome, RunResult};

pub(crate) async fn drive_invocation(
    run: RunInvocation<'_>,
) -> Result<RunResult, RunInfrastructureError> {
    let RunInvocation {
        project,
        host,
        options,
        input,
        owner,
    } = run;
    let body = if let Some(cause) = owner.scope.signal()?.cause()? {
        BodyResult {
            outcome: RunOutcome::Cancelled(cause),
            diagnostics: vec![],
            events: vec![],
            checkpoints: vec![],
        }
    } else {
        match input {
            InvocationInput::Run {
                entry,
                args,
                prepared,
            } => {
                run_checked_inner(
                    project,
                    entry,
                    args,
                    prepared,
                    host,
                    options,
                    owner.scope.clone(),
                )
                .await
            }
            InvocationInput::Resume {
                checkpoint,
                prepared,
            } => {
                resume_checkpoint_inner(
                    project,
                    checkpoint,
                    prepared,
                    host,
                    options,
                    owner.scope.clone(),
                )
                .await
            }
            InvocationInput::ResumeSource(checkpoint) => {
                super::resume_source_inner(project, checkpoint, host, options, owner.scope.clone())
                    .await
            }
        }
    };
    super::shutdown::settle(owner, body).await
}

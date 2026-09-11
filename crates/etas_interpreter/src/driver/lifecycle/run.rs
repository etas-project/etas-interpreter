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
            InvocationInput::Run { entry, args } => {
                run_checked_inner(project, entry, args, host, options, owner.scope.clone()).await
            }
            InvocationInput::Resume(checkpoint) => {
                resume_checkpoint_inner(project, checkpoint, host, options, owner.scope.clone())
                    .await
            }
        }
    };
    super::shutdown::settle(owner, body).await
}

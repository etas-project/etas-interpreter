use super::BodyResult;
use crate::api::{RunInfrastructureError, RunOutcome, RunResult, lifecycle::InvocationOwner};
use etas_host::execution::ScopeOutcome;

pub(super) async fn settle(
    mut owner: InvocationOwner,
    mut body: BodyResult,
) -> Result<RunResult, RunInfrastructureError> {
    owner
        .scope
        .finish_body(!matches!(body.outcome, RunOutcome::Failed(_)))?;
    owner.body_finished = true;
    let termination = owner.scope.join().await?;
    // A failure may initiate stop; cancellation never erases that primary failure.
    if !matches!(body.outcome, RunOutcome::Failed(_))
        && let ScopeOutcome::Cancelled(cause) = termination.outcome()
    {
        body.outcome = RunOutcome::Cancelled(cause.clone());
    }
    Ok(RunResult {
        outcome: body.outcome,
        termination,
        diagnostics: body.diagnostics,
        events: body.events,
        checkpoints: body.checkpoints,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{InterpValue, RunFailure};
    use etas_host::{
        HostRequestId, TraceContext, TraceId,
        execution::{CancellationReason, ExecutionScope, ExternalOutcome, ScopeState},
    };

    #[tokio::test(flavor = "current_thread")]
    async fn body_failure_remains_primary_while_owned_cleanup_drains() {
        for failed in [false, true] {
            let scope = ExecutionScope::new_owned();
            let operation = scope
                .register(None, Some(HostRequestId(1)), TraceContext::root(TraceId(1)))
                .unwrap();
            operation.begin_dispatch().unwrap();
            let body = BodyResult {
                outcome: if failed {
                    RunOutcome::Failed(RunFailure::Language(Box::new(
                        etas_core::Diagnostic::analysis(
                            etas_core::AnalysisDiagnosticCode::UnhandledRuntimeError,
                            etas_core::Span::empty(
                                etas_core::SourceId(7),
                                etas_core::TextSize::ZERO,
                            ),
                            "primary language failure",
                        ),
                    )))
                } else {
                    RunOutcome::Completed(InterpValue::Unit)
                },
                diagnostics: vec![],
                events: vec![],
                checkpoints: vec![],
            };
            let future = settle(
                InvocationOwner {
                    scope: scope.clone(),
                    body_finished: false,
                },
                body,
            );
            tokio::pin!(future);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(1), &mut future)
                    .await
                    .is_err()
            );
            assert_eq!(
                scope.state().unwrap(),
                if failed {
                    ScopeState::Stopping
                } else {
                    ScopeState::Draining
                }
            );
            assert!(scope.termination().unwrap().is_none());
            scope
                .cancel_source()
                .stop(CancellationReason::Requested)
                .unwrap();
            operation
                .complete(ExternalOutcome::Unknown, vec![])
                .unwrap();
            let result = future.await.unwrap();
            if failed {
                assert!(matches!(
                    result.outcome,
                    RunOutcome::Failed(RunFailure::Language(_))
                ));
            } else {
                assert!(matches!(result.outcome, RunOutcome::Cancelled(_)));
            }
            assert_eq!(
                result.termination.operations()[0].outcome(),
                Some(&ExternalOutcome::Unknown)
            );
        }
    }
}

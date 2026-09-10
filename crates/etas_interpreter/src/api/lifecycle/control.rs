use etas_host::{
    HostError,
    execution::{CancellationReason, ExecutionScope, ScopeState, StopWait, TerminationReport},
};

/// Observation does not drive execution. The embedding caller owns the run future.
#[derive(Clone, Debug)]
pub struct RunControl {
    scope: ExecutionScope,
}

impl RunControl {
    pub(crate) fn new(scope: ExecutionScope) -> Self {
        Self { scope }
    }
    pub fn status(&self) -> Result<ScopeState, HostError> {
        self.scope.state()
    }
    pub fn stop(&self, reason: CancellationReason) -> Result<(), HostError> {
        self.scope.cancel_source().stop(reason)
    }
    pub async fn join(&self) -> Result<TerminationReport, HostError> {
        self.scope.join().await
    }
    pub async fn wait_stopped(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<StopWait, HostError> {
        self.scope.wait_stopped(deadline).await
    }
}

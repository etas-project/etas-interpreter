use etas_host::{HostRequestId, StorageOperationRef};

use crate::{
    api::{ExecutionLimits, HostExecutionContext, ModelExecutionPolicy},
    orchestration::{
        ActiveHandlerRecord, CompletedHostBoundary, InterpreterCheckpoint, ResourceVersionRecord,
        RetryAttemptRecord, StorageWriteRecord,
    },
};

#[cfg(test)]
mod tests;

/// One invocation owns this state. Program/revision evaluation scopes borrow it;
/// changing checked code must not create fresh fuel, grants, IDs, or ledgers.
pub struct EvalRunState {
    pub(crate) replay_keys: super::replay::RunReplayKeys,
    pub storage_limits: etas_host::StorageLimits,
    pub execution: etas_host::execution::ExecutionScope,
    pub host_context: HostExecutionContext,
    pub model_policy: ModelExecutionPolicy,
    pub(crate) locked_model: Option<etas_host::ModelName>,
    pub execution_limits: ExecutionLimits,
    pub current_session: Option<String>,
    pub diagnostics: Vec<etas_core::Diagnostic>,
    pub events: crate::api::lifecycle::EventLog,
    pub checkpoints: Vec<InterpreterCheckpoint>,
    pub(super) next_step: u32,
    pub(super) next_checkpoint: u32,
    pub(super) next_retry: u32,
    pub(super) next_handler_scope: u32,
    pub(super) next_host_request: u32,
    pub(super) next_message: u32,
    pub(super) safe_points: super::safe_point::ExecutionSafePointScheduler,
    pub(super) host_trace_digest_key: Result<etas_host::HostTraceDigestKey, etas_host::HostError>,
    pub(super) retry_stack: Vec<RetryAttemptRecord>,
    pub(super) handler_stack: Vec<ActiveHandlerRecord>,
    pub(super) completed_host_boundaries: std::collections::BTreeMap<
        crate::orchestration::BoundaryOccurrenceId,
        CompletedHostBoundary,
    >,
    pub(super) resource_versions: Vec<ResourceVersionRecord>,
    pub(crate) storage_identity: Result<etas_host::StorageOperationKey, etas_host::HostError>,
    pub(crate) storage_writes: Vec<StorageWriteRecord>,
    pub(crate) storage_operations: std::collections::BTreeMap<u32, StorageOperationRef>,
}

impl EvalRunState {
    pub(super) fn from_input(input: super::EvalRunStateInput) -> Self {
        let limits = input.storage_limits;
        let receipt_retention = limits.max_receipt_retention_seconds;
        Self {
            replay_keys: input.replay_keys,
            storage_limits: limits,
            execution: input.execution,
            host_context: input.host_context,
            locked_model: input
                .model_policy
                .model_locked
                .then(|| input.model_policy.model.clone()),
            model_policy: input.model_policy,
            execution_limits: input.execution_limits,
            current_session: input.current_session,
            diagnostics: Vec::new(),
            events: crate::api::lifecycle::EventLog::new(input.event_observer),
            checkpoints: Vec::new(),
            next_step: 0,
            next_checkpoint: 0,
            next_retry: 0,
            next_handler_scope: 0,
            next_host_request: 0,
            next_message: 0,
            safe_points: super::safe_point::ExecutionSafePointScheduler::new(input.consumed_steps),
            host_trace_digest_key: etas_host::HostTraceDigestKey::generate(),
            retry_stack: Vec::new(),
            handler_stack: Vec::new(),
            completed_host_boundaries: Default::default(),
            resource_versions: Vec::new(),
            storage_identity: etas_host::StorageOperationKey::new(std::time::Duration::from_secs(
                receipt_retention,
            )),
            storage_writes: Vec::new(),
            storage_operations: Default::default(),
        }
    }

    pub(super) fn next_request_id(&mut self) -> HostRequestId {
        let id = HostRequestId(self.next_host_request);
        self.next_host_request += 1;
        id
    }
}

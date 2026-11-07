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
pub(crate) struct EvalRunState {
    pub(crate) shared: EvalSharedState,
    pub(crate) root_task: TaskLocalState,
    pub(crate) scheduler: crate::driver::scheduler::TaskScheduler,
}

/// State shared by every task in one invocation.
pub(crate) struct EvalSharedState {
    pub(crate) replay_keys: super::replay::RunReplayKeys,
    pub storage_limits: etas_host::StorageLimits,
    pub host_context: HostExecutionContext,
    pub(crate) locked_model: Option<etas_host::ModelName>,
    pub execution_limits: ExecutionLimits,
    // Shared budgets report the invocation site, independent of which task
    // happens to observe exhaustion first.
    pub(crate) limit_span: etas_core::Span,
    pub(crate) cpu_executor: std::sync::Arc<dyn crate::driver::cpu::CpuExecutor>,
    pub(crate) cpu_min_work: u64,
    pub(crate) cpu_programs: std::collections::HashMap<
        (usize, etas_hir::HirItemId),
        std::sync::Arc<crate::driver::cpu::PreparedCpuProgram>,
    >,
    pub(crate) cpu_jobs: std::collections::BTreeMap<crate::driver::cpu::CpuJobId, CpuJobRecord>,
    pub(crate) next_cpu_dispatch: u64,
    pub(crate) next_cpu_completion: u64,
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
    pub(super) completed_host_boundaries: std::collections::BTreeMap<
        crate::orchestration::BoundaryOccurrenceId,
        CompletedHostBoundary,
    >,
    pub(super) resource_versions: Vec<ResourceVersionRecord>,
    pub(crate) storage_identity: Result<etas_host::StorageOperationKey, etas_host::HostError>,
    pub(crate) storage_writes: Vec<StorageWriteRecord>,
    pub(crate) storage_operations: std::collections::BTreeMap<u32, StorageOperationRef>,
}

pub(crate) struct CpuJobRecord {
    pub(crate) task: crate::driver::scheduler::TaskHandle,
    pub(crate) cancellation: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Execution stacks owned by one cooperatively scheduled task.
pub(crate) struct TaskLocalState {
    pub(crate) identity: crate::driver::scheduler::TaskIdentity,
    pub(crate) parent: Option<crate::driver::scheduler::TaskIdentity>,
    pub(crate) task_scope: crate::driver::scheduler::TaskScopeId,
    pub(crate) execution: etas_host::execution::ExecutionScope,
    pub(crate) cancellation: Result<etas_host::execution::CancelSignal, etas_host::HostError>,
    pub(crate) model_policy: ModelExecutionPolicy,
    pub(super) retry_stack: Vec<RetryAttemptRecord>,
    pub(super) handler_stack: Vec<ActiveHandlerRecord>,
    /// Live residual monitor bookkeeping for this task's program scopes.
    pub(crate) residual: crate::eval::residual::ResidualRunState,
}

impl TaskLocalState {
    pub(crate) fn dynamic_depths(&self) -> (usize, usize) {
        (self.handler_stack.len(), self.retry_stack.len())
    }
    pub(crate) fn root(
        task_scope: crate::driver::scheduler::TaskScopeId,
        execution: etas_host::execution::ExecutionScope,
        model_policy: ModelExecutionPolicy,
    ) -> Self {
        Self {
            identity: crate::driver::scheduler::TaskIdentity::Root,
            parent: None,
            task_scope,
            cancellation: execution.signal(),
            execution,
            model_policy,
            retry_stack: Vec::new(),
            handler_stack: Vec::new(),
            residual: Default::default(),
        }
    }

    /// Fork task-local dynamic state while keeping authority and budgets under
    /// the existing host execution tree. The child owns its stacks after this
    /// point; later handler/retry mutations cannot leak into its parent.
    pub(crate) fn child_in_scope(
        &self,
        task_scope: crate::driver::scheduler::TaskScopeId,
        identity: crate::driver::scheduler::TaskId,
    ) -> Result<Self, etas_host::HostError> {
        let execution = self.execution.child()?;
        execution.claim_body()?;
        Ok(Self {
            identity: crate::driver::scheduler::TaskIdentity::Scheduled(identity),
            parent: Some(self.identity),
            task_scope,
            cancellation: execution.signal(),
            execution,
            model_policy: self.model_policy.clone(),
            retry_stack: self.retry_stack.clone(),
            handler_stack: self.handler_stack.clone(),
            residual: Default::default(),
        })
    }
}

impl std::ops::Deref for EvalRunState {
    type Target = EvalSharedState;

    fn deref(&self) -> &Self::Target {
        &self.shared
    }
}

impl std::ops::DerefMut for EvalRunState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.shared
    }
}

impl EvalSharedState {
    #[cfg(test)]
    pub(crate) fn consumed_steps(&self) -> u64 {
        self.safe_points.consumed_steps()
    }
}

impl EvalRunState {
    pub(super) fn from_input(input: super::EvalRunStateInput) -> Self {
        let limits = input.storage_limits;
        let receipt_retention = limits.max_receipt_retention_seconds;
        let locked_model = input
            .model_policy
            .model_locked
            .then(|| input.model_policy.model.clone());
        let root_scope = crate::driver::scheduler::TaskScopeId(0);
        let root_task = TaskLocalState::root(root_scope, input.execution, input.model_policy);
        let mut events = crate::api::lifecycle::EventLog::new(input.event_observer);
        let executor = match input.runtime {
            Some(runtime) => Ok(runtime.executor(input.cpu_threads)),
            None => crate::driver::cpu::new_executor(input.cpu_threads),
        };
        let cpu_executor = match executor {
            Ok(executor) => executor,
            Err(error) => {
                events.push(crate::orchestration::WorkflowEvent::CpuExecutorFallback {
                    task: None,
                    message: error.to_string(),
                });
                std::sync::Arc::new(crate::driver::cpu::InlineCpuExecutor::new())
            }
        };
        let shared = EvalSharedState {
            replay_keys: input.replay_keys,
            storage_limits: limits,
            host_context: input.host_context,
            locked_model,
            execution_limits: input.execution_limits,
            limit_span: input.limit_span,
            cpu_executor,
            cpu_min_work: input.cpu_min_work,
            cpu_programs: Default::default(),
            cpu_jobs: Default::default(),
            next_cpu_dispatch: 0,
            next_cpu_completion: 0,
            current_session: input.current_session,
            diagnostics: Vec::new(),
            events,
            checkpoints: Vec::new(),
            next_step: 0,
            next_checkpoint: 0,
            next_retry: 0,
            next_handler_scope: 0,
            next_host_request: 0,
            next_message: 0,
            safe_points: super::safe_point::ExecutionSafePointScheduler::new(input.consumed_steps),
            host_trace_digest_key: etas_host::HostTraceDigestKey::generate(),
            completed_host_boundaries: Default::default(),
            resource_versions: Vec::new(),
            storage_identity: etas_host::StorageOperationKey::new(std::time::Duration::from_secs(
                receipt_retention,
            )),
            storage_writes: Vec::new(),
            storage_operations: Default::default(),
        };
        let scheduler = crate::driver::scheduler::TaskScheduler::new(
            root_scope,
            std::num::NonZeroUsize::new(crate::driver::scheduler::MAX_CONCURRENCY)
                .expect("scheduler ceiling is non-zero"),
        );
        Self {
            shared,
            root_task,
            scheduler,
        }
    }
}

impl EvalSharedState {
    pub(super) fn next_request_id(&mut self) -> HostRequestId {
        let id = HostRequestId(self.next_host_request);
        self.next_host_request += 1;
        id
    }
}

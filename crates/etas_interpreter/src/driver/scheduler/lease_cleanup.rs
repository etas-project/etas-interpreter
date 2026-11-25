use super::*;
use crate::control::{CallPurity, CallTarget, Continuation, ControlSignal, PendingCall};
use crate::value::HostHandleValue;
use etas_host::{execution::ExecutionScope, session::SessionLeaseScope};

pub(super) struct LeaseCleanup {
    owner: ExecutionScope,
    cleanup: ExecutionScope,
    release: CallTarget,
    span: etas_core::Span,
    current: Option<(SessionLeaseScope, HostHandleValue)>,
    body_outcome: Option<TaskOutcome>,
}

impl LeaseCleanup {
    pub(super) fn running(&self) -> bool {
        self.body_outcome.is_some()
    }

    pub(super) fn owner_execution(&self) -> &ExecutionScope {
        &self.owner
    }
}

impl Drop for LeaseCleanup {
    fn drop(&mut self) {
        // Releasing the reserved body allows the native supervisor to fence any
        // generation whose release was not confirmed, including a lost handoff.
        let _ = self.cleanup.finish_body(true);
    }
}

impl TaskScheduler {
    pub(crate) fn configure_lease_cleanup(
        &mut self,
        owner: TaskHandle,
        release: CallTarget,
        span: etas_core::Span,
    ) -> Result<(), SchedulerError> {
        let execution = self.tasks[&owner.task].local.execution.clone();
        let cleanup = execution.reserve_cleanup_body()?;
        self.lease_cleanups.insert(
            owner.task,
            LeaseCleanup {
                owner: execution,
                cleanup,
                release,
                span,
                current: None,
                body_outcome: None,
            },
        );
        Ok(())
    }

    pub(crate) fn confirm_lease_generation(
        &mut self,
        acquisition_owner: Option<TaskHandle>,
        scope: &SessionLeaseScope,
        handle: &HostHandleValue,
    ) {
        if let Some(owner) = acquisition_owner {
            if let Some(cleanup) = self.lease_cleanups.get_mut(&owner.task)
                && cleanup.current.is_none()
            {
                cleanup.current = Some((scope.clone(), handle.clone()));
            }
            return;
        }
        // Handoff can run in a different Agent or a nested resource scope.
        // The original acquisition still owns the chain and the callback ABI.
        for cleanup in self.lease_cleanups.values_mut() {
            if let Some((chain, current)) = &mut cleanup.current
                && chain == scope
            {
                *current = handle.clone().into_revision_type(current.nominal_type());
                break;
            }
        }
    }

    pub(super) fn process_lease_cleanup(
        &mut self,
        handle: TaskHandle,
        outcome: &mut TaskOutcome,
    ) -> Result<bool, SchedulerError> {
        let Some(cleanup) = self.lease_cleanups.get_mut(&handle.task) else {
            return Ok(false);
        };
        let task = self.tasks.get_mut(&handle.task).expect("lease owner");
        if task.local.host_operations.get() != 0 {
            return Ok(false);
        }
        if let Some(body_outcome) = cleanup.body_outcome.take() {
            task.local.execution = cleanup.owner.clone();
            task.local.cancellation = task.local.execution.signal();
            if matches!(outcome, TaskOutcome::Completed(_))
                || !matches!(body_outcome, TaskOutcome::Completed(_))
            {
                *outcome = body_outcome;
            }
            self.lease_cleanups.remove(&handle.task);
            return Ok(false);
        }
        let Some((_, lease)) = cleanup
            .current
            .take()
            .filter(|(_, lease)| lease.live_session_lease())
        else {
            self.lease_cleanups.remove(&handle.task);
            return Ok(false);
        };
        cleanup.body_outcome = Some(outcome.clone());
        task.local.execution = cleanup.cleanup.clone();
        task.local.cancellation = cleanup.cleanup.signal();
        task.machine = EvalMachine::new();
        task.machine
            .resume(ControlSignal::pending_call(PendingCall {
                target: cleanup.release.clone(),
                purity: CallPurity::Ordinary,
                site: None,
                args: vec![InterpValue::HostHandle(lease)],
                span: cleanup.span,
                continuation: Continuation::BlockValue,
            }));
        task.waiting_continuation = None;
        self.wakeups.remove(&handle.task);
        if task.state != TaskState::Ready {
            task.state = TaskState::Ready;
            self.enqueue(handle.task);
        }
        Ok(true)
    }
}

use super::*;

impl<'a> EvalContext<'a> {
    pub(crate) fn execute_from_checkpoint_signal(
        &mut self,
        checkpoint: &InterpreterCheckpoint,
    ) -> ControlSignal {
        self.handler_stack = checkpoint.handlers.handlers.clone();
        self.retry_stack = checkpoint.retry_state.attempts.clone();
        self.completed_host_boundaries = checkpoint.completed_host_boundaries.completed.clone();
        self.resource_versions = checkpoint.resource_versions.versions.clone();
        self.next_checkpoint = checkpoint.id.0 + 1;
        self.next_retry = self
            .retry_stack
            .iter()
            .map(|attempt| attempt.id.0 + 1)
            .max()
            .unwrap_or(0);
        self.next_handler_scope = self
            .handler_stack
            .iter()
            .map(|handler| handler.id.0 + 1)
            .max()
            .unwrap_or(0);
        self.next_step = checkpoint.trace.events_recorded as u32;
        self.next_host_request = checkpoint.trace.next_host_request;
        self.next_message = checkpoint.trace.next_message;
        self.storage_identity = Ok(checkpoint.storage.identity.clone());
        self.storage_writes = checkpoint.storage.writes.clone();
        self.storage_operations = checkpoint.storage.operations.clone();
        ControlSignal::Value(InterpValue::Unit)
    }

    pub(super) fn record_checkpoint(
        &mut self,
        label: Option<String>,
        machine: crate::orchestration::MachineSnapshot,
    ) -> Result<(), crate::control::ExecutionFault> {
        let id = CheckpointId(self.next_checkpoint);
        let compilation = crate::orchestration::CheckpointCompilationIdentity::for_project(
            self.checked,
            self.entry_item,
        )
        .map_err(|message| {
            crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                item_span(self.checked, self.entry_item),
                format!("checkpoint compilation identity is unavailable: {message}"),
            )
        })?;
        let budget =
            crate::orchestration::CheckpointBudgetSnapshot::capture(&self.host_context.budget)
                .map_err(|error| {
                    crate::control::ExecutionFault::new(
                        AnalysisDiagnosticCode::UnhandledRuntimeError,
                        item_span(self.checked, self.entry_item),
                        format!("checkpoint budget snapshot failed: {error}"),
                    )
                })?;
        let storage_identity = self.storage_identity.clone().map_err(|error| {
            crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                item_span(self.checked, self.entry_item),
                error.to_string(),
            )
        })?;
        self.next_checkpoint += 1;
        self.events.push(WorkflowEvent::CheckpointCreated(id));
        self.checkpoints.push(InterpreterCheckpoint {
            storage: crate::orchestration::StorageSnapshot {
                identity: storage_identity,
                writes: self.storage_writes.clone(),
                operations: self.storage_operations.clone(),
            },
            id,
            label,
            compilation,
            entry_item: self.entry_item,
            args: self.entry_args.to_vec(),
            machine,
            handlers: HandlerSnapshot {
                handlers: self.handler_stack.clone(),
            },
            retry_state: RetrySnapshot {
                attempts: self.retry_stack.clone(),
            },
            trace: TraceSnapshot {
                events_recorded: self.events.len(),
                next_message: self.next_message,
                next_host_request: self.next_host_request,
            },
            execution_progress: ExecutionProgressSnapshot {
                consumed_steps: self.safe_points.consumed_steps(),
                original_limits: self.execution_limits,
            },
            host_state: crate::orchestration::CheckpointHostState {
                trace: self.host_context.trace.clone(),
                budget,
            },
            current_session: self.current_session.clone(),
            resource_versions: ResourceVersionSnapshot {
                versions: self.resource_versions.clone(),
            },
            completed_host_boundaries: HostBoundaryLedger {
                completed: self.completed_host_boundaries.clone(),
            },
        });
        Ok(())
    }

    pub(super) fn step_id(&mut self) -> WorkflowStepId {
        let id = WorkflowStepId(self.next_step);
        self.next_step += 1;
        id
    }
}

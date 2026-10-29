use super::*;

impl<'a> EvalContext<'a> {
    pub(crate) fn seed_resume_counters(&mut self, trace: &TraceSnapshot) -> Result<(), String> {
        let step = u32::try_from(trace.events_recorded)
            .map_err(|_| "checkpoint trace step counter exceeds the runtime ABI")?;
        self.next_step = self.next_step.max(step);
        self.next_host_request = self.next_host_request.max(trace.next_host_request);
        self.next_message = self.next_message.max(trace.next_message);
        Ok(())
    }

    pub(crate) fn execute_from_checkpoint_signal(
        &mut self,
        checkpoint: &InterpreterCheckpoint,
    ) -> ControlSignal {
        self.model_policy =
            crate::eval::machine::snapshot::restore_model_policy(checkpoint.model_policy.clone());
        self.handler_stack = checkpoint.handlers.handlers.clone();
        self.retry_stack = checkpoint.retry_state.attempts.clone();
        let mut boundaries = std::collections::BTreeMap::new();
        for boundary in &checkpoint.completed_host_boundaries.completed {
            if boundaries
                .insert(boundary.occurrence.clone(), boundary.clone())
                .is_some()
            {
                return ControlSignal::missing_checked_fact(
                    "checkpoint has duplicate completed boundary occurrences",
                    item_span(self.checked, self.entry_item),
                );
            }
        }
        self.completed_host_boundaries = boundaries;
        self.resource_versions = checkpoint.resource_versions.versions.clone();
        self.next_checkpoint = checkpoint.id.0 + 1;
        self.next_retry = self
            .retry_stack
            .iter()
            .chain(
                checkpoint
                    .machine
                    .suspended_programs
                    .iter()
                    .flat_map(|node| &node.retries.attempts),
            )
            .map(|attempt| attempt.id.0 + 1)
            .max()
            .unwrap_or(0);
        self.next_handler_scope = self
            .handler_stack
            .iter()
            .chain(
                checkpoint
                    .machine
                    .suspended_programs
                    .iter()
                    .flat_map(|node| &node.handlers.handlers),
            )
            .map(|handler| handler.id.0 + 1)
            .max()
            .unwrap_or(0);
        if let Err(message) = self.seed_resume_counters(&checkpoint.trace) {
            return ControlSignal::missing_checked_fact(
                message,
                item_span(self.checked, self.entry_item),
            );
        }
        self.storage_identity = Ok(checkpoint.storage.identity.clone());
        self.storage_writes = checkpoint.storage.writes.clone();
        self.storage_operations = checkpoint.storage.operations.clone();
        ControlSignal::Value(InterpValue::Unit)
    }

    pub(super) fn record_checkpoint(
        &mut self,
        label: Option<String>,
        mut captured: crate::eval::machine::snapshot::CapturedMachineSnapshot,
    ) -> Result<(), crate::control::ExecutionFault> {
        let root = captured.root.take().unwrap_or_else(|| {
            crate::eval::machine::snapshot::CheckpointRoot {
                program: self.checked_execution_program(),
                args: self.entry_args.to_vec(),
            }
        });
        let checked = &root.program.checked;
        let plan = &root.program.plan;
        let entry = plan.entry.item;
        let id = CheckpointId(self.next_checkpoint);
        let compilation =
            crate::orchestration::CheckpointCompilationIdentity::for_project(checked, entry)
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
        if let Some(revision) = root.program.revision.as_ref() {
            captured
                .revisions
                .insert(revision.revision().key(), revision.clone());
        }
        let revisions = if let Some(revision) = captured.revisions.values().next() {
            let mut set = crate::api::code::PreparedRevisionSet::new(
                revision.type_bridge().original_program().clone(),
            );
            for revision in captured.revisions.values() {
                set.insert(revision.clone()).map_err(|message| {
                    crate::control::ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        item_span(self.checked, self.entry_item),
                        message,
                    )
                })?;
            }
            Some(set)
        } else {
            None
        };
        let checkpoint = InterpreterCheckpoint {
            retained_code: Some(std::sync::Arc::new(
                crate::api::code::CheckpointCodePins::new(captured.revisions),
            )),
            storage: crate::orchestration::StorageSnapshot {
                identity: storage_identity,
                writes: self.storage_writes.clone(),
                operations: self.storage_operations.clone(),
            },
            id,
            label,
            compilation,
            code_revision: root.program.revision.as_ref().map(|revision| {
                crate::orchestration::CheckpointCodeRevision::for_revision(revision)
            }),
            entry_item: entry,
            args: captured.args,
            machine: captured.machine,
            model_policy: crate::eval::machine::snapshot::capture_model_policy(&self.model_policy),
            handlers: HandlerSnapshot {
                handlers: self.handler_stack.clone(),
            },
            retry_state: RetrySnapshot {
                attempts: self.retry_stack.clone(),
            },
            trace: TraceSnapshot {
                events_recorded: self.events.len() + 1,
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
                completed: self.completed_host_boundaries.values().cloned().collect(),
            },
        };
        crate::eval::machine::snapshot::SnapshotValidator::new(
            checked,
            &plan.slots,
            &plan.dispatch,
            &plan.closures,
            &self.storage_limits,
        )
        .with_execution_revision(root.program.revision.as_deref())
        .with_revisions(revisions.as_ref())
        .validate_checkpoint(&checkpoint)
        .map_err(|message| {
            crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                item_span(self.checked, self.entry_item),
                format!("checkpoint cannot preserve checked execution state: {message}"),
            )
        })?;
        self.next_checkpoint += 1;
        self.events.push(WorkflowEvent::CheckpointCreated(id));
        self.checkpoints.push(checkpoint);
        Ok(())
    }

    pub(super) fn step_id(&mut self) -> WorkflowStepId {
        let id = WorkflowStepId(self.next_step);
        self.next_step += 1;
        id
    }
}

use std::{collections::BTreeMap, sync::Arc};

use crate::{
    control::{Continuation, PendingPerform},
    driver::scheduler::{TaskIdentity, TaskOutcome, TaskResume, TaskState, TaskWait, TaskWaiter},
    eval::machine::{
        EvalMachine,
        snapshot::{CaptureContext, CapturedMachineSnapshot, CheckpointRoot, SharedFrameCapture},
    },
    orchestration::{
        AuxiliaryContinuationSnapshot, HandlerSnapshot, RetrySnapshot, ScheduledTaskSnapshot,
        ScheduledTaskStateSnapshot, TaskCancellationSnapshot, TaskLocalSnapshot,
        TaskOutcomeSnapshot, TaskResumeSnapshot, TaskSchedulerSnapshot,
    },
    value::CheckedExecutionProgram,
};

pub(crate) struct CapturedTaskCheckpoint {
    pub root: CapturedMachineSnapshot,
    pub scheduler: TaskSchedulerSnapshot,
}

#[derive(Default)]
struct TaskCapture {
    context: CaptureContext,
    auxiliary: Vec<AuxiliaryContinuationSnapshot>,
}

impl TaskCapture {
    fn continuation(
        &mut self,
        program: &CheckedExecutionProgram,
        continuation: &Continuation,
    ) -> Result<u32, String> {
        let index =
            u32::try_from(self.auxiliary.len()).map_err(|_| "too many task continuations")?;
        let program = program.checkpoint_identity(&mut self.context.revisions);
        let continuation = self.context.continuation(continuation)?;
        self.auxiliary.push(AuxiliaryContinuationSnapshot {
            program,
            continuation,
        });
        Ok(index)
    }

    fn resume(
        &mut self,
        program: &CheckedExecutionProgram,
        resume: &TaskResume,
    ) -> Result<TaskResumeSnapshot, String> {
        Ok(match resume {
            TaskResume::Await(continuation) => {
                TaskResumeSnapshot::Await(self.continuation(program, continuation)?)
            }
            TaskResume::Run(continuation) => {
                TaskResumeSnapshot::Run(self.continuation(program, continuation)?)
            }
            TaskResume::Transfer {
                token,
                continuation,
                output,
                span,
            } => TaskResumeSnapshot::Transfer {
                token: *token,
                continuation: self.continuation(program, continuation)?,
                output: *output,
                span: *span,
            },
        })
    }
}

impl super::EvalContext<'_> {
    /// Capture only after the coordinator has committed all admitted work.
    /// The shared frame table preserves aliases across tasks and their handles.
    pub(crate) fn capture_task_checkpoint(
        &self,
        machine: &EvalMachine,
    ) -> Result<CapturedTaskCheckpoint, String> {
        assert!(
            self.cpu_jobs.is_empty(),
            "CPU completions must be committed before capture"
        );
        let tasks = self.scheduler.checkpoint_tasks();
        let frames = SharedFrameCapture::default();
        let mut root_capture = TaskCapture {
            context: CaptureContext::for_tasks(&frames),
            ..Default::default()
        };
        let root = machine
            .checkpoint_root()
            .cloned()
            .unwrap_or_else(|| CheckpointRoot {
                program: self.checked_execution_program(),
                args: self.entry_args.to_vec(),
            });
        let mut captures = tasks
            .keys()
            .map(|id| {
                (
                    *id,
                    TaskCapture {
                        context: CaptureContext::for_tasks(&frames),
                        ..Default::default()
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut revisions = BTreeMap::new();
        let continuations = self.scheduler.capture_continuations(
            &mut revisions,
            |origin, program, continuation| {
                let program =
                    program.ok_or("task continuation has no checked execution program")?;
                match origin {
                    TaskIdentity::Root => root_capture.continuation(program, continuation),
                    TaskIdentity::Scheduled(id) => captures
                        .get_mut(&id)
                        .ok_or("task continuation has no origin machine")?
                        .continuation(program, continuation),
                }
            },
        )?;
        let mut scope_owners = BTreeMap::new();
        scope_owners.insert(self.task.execution.id().as_u64(), TaskIdentity::Root);
        for (id, task) in tasks {
            scope_owners.insert(
                task.local.execution.id().as_u64(),
                TaskIdentity::Scheduled(*id),
            );
        }
        let cancellation =
            |cause: &etas_host::execution::CancellationCause| TaskCancellationSnapshot {
                source: scope_owners
                    .get(&cause.origin().as_u64())
                    .copied()
                    .unwrap_or(TaskIdentity::Root),
                reason: cause.reason().into(),
            };
        let mut saved_tasks = BTreeMap::new();
        for (id, task) in tasks {
            let mut capture = captures
                .remove(id)
                .expect("capture context exists for every task");
            let program = task
                .program
                .as_ref()
                .ok_or("scheduled task has no checked execution program")?;
            let active = task
                .machine
                .program_scope()
                .map_or_else(|| program.clone(), |scope| scope.program);
            let waiting_continuation = task
                .waiting_continuation
                .as_ref()
                .map(|resume| capture.resume(&active, resume))
                .transpose()?;
            let outcome = task
                .outcome
                .as_ref()
                .map(|outcome| {
                    Ok::<_, String>(match outcome {
                        TaskOutcome::Completed(value) => {
                            TaskOutcomeSnapshot::Completed(capture.context.value(value)?)
                        }
                        TaskOutcome::Raised(perform) => TaskOutcomeSnapshot::Raised {
                            error_type: perform
                                .error_type
                                .ok_or("terminal task error has no checked error type")?,
                            payload: capture.context.value(
                                perform
                                    .args
                                    .first()
                                    .ok_or("terminal task error has no payload")?,
                            )?,
                            span: perform.span,
                        },
                        TaskOutcome::Failed(failure) => {
                            TaskOutcomeSnapshot::Failed(failure.clone())
                        }
                        TaskOutcome::Cancelled(cause) => {
                            TaskOutcomeSnapshot::Cancelled(cancellation(cause))
                        }
                    })
                })
                .transpose()?;
            let saved_machine = if task.state.is_terminal() {
                if !capture.auxiliary.is_empty() {
                    return Err("terminal task still owns an executable continuation".into());
                }
                None
            } else {
                let mut saved = task.machine.capture_machine_in_program(
                    &mut capture.context,
                    task.local.residual.counters(),
                    Some(program),
                )?;
                saved.auxiliary = capture.auxiliary;
                saved.residual.exits = task.local.residual.log.snapshot(&program.plan.residual)?;
                saved.residual.validate_counters()?;
                Some(saved)
            };
            let state = match task.state {
                TaskState::Ready => ScheduledTaskStateSnapshot::Ready,
                TaskState::Waiting(TaskWait::Task(id)) => {
                    ScheduledTaskStateSnapshot::WaitingTask(id)
                }
                TaskState::Waiting(TaskWait::Scope(scope)) => {
                    ScheduledTaskStateSnapshot::WaitingScope(scope)
                }
                TaskState::Waiting(TaskWait::Resource(id)) => {
                    ScheduledTaskStateSnapshot::WaitingResource(id.0)
                }
                TaskState::Completed => ScheduledTaskStateSnapshot::Completed,
                TaskState::Raised => ScheduledTaskStateSnapshot::Raised,
                TaskState::Failed => ScheduledTaskStateSnapshot::Failed,
                TaskState::Cancelled => ScheduledTaskStateSnapshot::Cancelled,
                TaskState::Running | TaskState::Waiting(TaskWait::Host(_) | TaskWait::Cpu(_)) => {
                    return Err("task has not reached the coordinator checkpoint barrier".into());
                }
            };
            let cause = task
                .local
                .cancellation
                .as_ref()
                .map_err(ToString::to_string)?
                .cause()
                .map_err(|error| error.to_string())?;
            saved_tasks.insert(
                *id,
                ScheduledTaskSnapshot {
                    scope: task.scope,
                    program: program.checkpoint_identity(&mut capture.context.revisions),
                    local: TaskLocalSnapshot {
                        parent: task
                            .local
                            .parent
                            .ok_or("scheduled task has no logical parent")?,
                        model_policy: super::machine::snapshot::capture_model_policy(
                            &task.local.model_policy,
                        ),
                        handlers: HandlerSnapshot {
                            handlers: task.local.handler_stack.clone(),
                        },
                        retries: RetrySnapshot {
                            attempts: task.local.retry_stack.clone(),
                        },
                        cancellation: cause.as_ref().map(cancellation),
                    },
                    machine: saved_machine,
                    handler_floor: task.handler_floor,
                    state,
                    outcome,
                    waiters: task
                        .waiters
                        .iter()
                        .map(|TaskWaiter::Task(id)| *id)
                        .collect(),
                    waiting_continuation,
                    source_index: task.source_index,
                    active_slot: task.active_slot,
                    cpu_pending: task.cpu_capsule.is_some(),
                    resource_keys: task.resource_keys.clone(),
                },
            );
            revisions.extend(capture.context.revisions);
        }
        let args = root
            .args
            .iter()
            .map(|arg| root_capture.context.value(arg))
            .collect::<Result<_, _>>()?;
        let mut saved = machine.capture_machine_in_program(
            &mut root_capture.context,
            self.task.residual.counters(),
            Some(&root.program),
        )?;
        saved.auxiliary = root_capture.auxiliary;
        saved.residual.exits = self
            .task
            .residual
            .log
            .snapshot(&root.program.plan.residual)?;
        saved.residual.validate_counters()?;
        root.program.checkpoint_identity(&mut revisions);
        revisions.extend(root_capture.context.revisions);
        Ok(CapturedTaskCheckpoint {
            root: CapturedMachineSnapshot {
                root: Some(root),
                args,
                machine: saved,
                revisions,
            },
            scheduler: TaskSchedulerSnapshot {
                metadata: self.scheduler.capture_metadata(continuations)?,
                tasks: saved_tasks,
                counters: crate::orchestration::TaskRunCountersSnapshot {
                    step: self.next_step,
                    retry: self.next_retry,
                    handler_scope: self.next_handler_scope,
                    host_request: self.next_host_request,
                    message: self.next_message,
                    cpu_dispatch: self.next_cpu_dispatch,
                    cpu_completion: self.next_cpu_completion,
                },
                diagnostics: self.diagnostics.clone(),
            },
        })
    }

    pub(crate) fn restore_task_checkpoint(
        &mut self,
        saved: &TaskSchedulerSnapshot,
        root_snapshot: &crate::orchestration::MachineSnapshot,
        root: CheckpointRoot,
        revisions: Option<Arc<crate::api::code::PreparedRevisionSet>>,
        ordering: Option<super::machine::snapshot::ordering::OrderedRestoreChecks>,
    ) -> Result<EvalMachine, String> {
        use super::machine::snapshot::{
            RestoreContext, RestoredMachineSnapshot, SharedFrameRestore,
        };
        use crate::driver::scheduler::{ResourceGuardId, ScheduledTask, TaskScheduler};

        // Parents always precede their children in the monotonic task ID space.
        // Build the entire execution tree before finishing any terminal body.
        let mut locals = BTreeMap::new();
        for (id, task) in &saved.tasks {
            let parent = match task.local.parent {
                TaskIdentity::Root => &*self.task,
                TaskIdentity::Scheduled(parent) => locals
                    .get(&parent)
                    .ok_or("checkpoint task parent is missing or does not precede its child")?,
            };
            let mut local = parent
                .child_in_scope(task.scope, *id)
                .map_err(|error| error.to_string())?;
            local.model_policy =
                super::machine::snapshot::restore_model_policy(task.local.model_policy.clone());
            local.handler_stack = task.local.handlers.handlers.clone();
            local.retry_stack = task.local.retries.attempts.clone();
            locals.insert(*id, local);
        }
        let execution = |identity| match identity {
            TaskIdentity::Root => Ok(self.task.execution.clone()),
            TaskIdentity::Scheduled(id) => locals
                .get(&id)
                .map(|local| local.execution.clone())
                .ok_or("checkpoint cancellation origin has no task"),
        };
        for task in saved.tasks.values() {
            if let Some(cause) = &task.local.cancellation {
                execution(cause.source)?
                    .cancel_source()
                    .stop(cause.reason.into())
                    .map_err(|error| error.to_string())?;
            }
        }
        for (id, task) in &saved.tasks {
            if matches!(
                task.state,
                ScheduledTaskStateSnapshot::Completed
                    | ScheduledTaskStateSnapshot::Raised
                    | ScheduledTaskStateSnapshot::Failed
                    | ScheduledTaskStateSnapshot::Cancelled
            ) {
                locals[id]
                    .execution
                    .finish_body(!matches!(
                        task.state,
                        ScheduledTaskStateSnapshot::Raised | ScheduledTaskStateSnapshot::Failed
                    ))
                    .map_err(|error| error.to_string())?;
            }
        }
        let frames = SharedFrameRestore::default();
        let current = self.host_context.clone();
        let locked_model = self.locked_model.clone();
        let limits = self.storage_limits.clone();
        let mut root_restored = restore_machine(
            root_snapshot,
            &root.program,
            root.args.clone(),
            self.task,
            TaskIdentity::Root,
            &saved.metadata,
            &current,
            locked_model.clone(),
            &limits,
            revisions.clone(),
            ordering.clone(),
            frames.clone(),
        )?;
        let mut restored = BTreeMap::new();
        let mut tasks = BTreeMap::new();
        for (id, task) in &saved.tasks {
            let program = CheckedExecutionProgram::restore_identity(
                &task.program,
                &root.program,
                revisions.as_deref(),
            )?;
            let mut local = locals
                .remove(id)
                .expect("execution scope was rebuilt for every task");
            let mut machine = match &task.machine {
                Some(snapshot) => restore_machine(
                    snapshot,
                    &program,
                    Vec::new(),
                    &mut local,
                    TaskIdentity::Scheduled(*id),
                    &saved.metadata,
                    &self.host_context,
                    self.locked_model.clone(),
                    &self.storage_limits,
                    revisions.clone(),
                    ordering.clone(),
                    frames.clone(),
                )?,
                None => {
                    let context = RestoreContext::for_task(
                        program.clone(),
                        &self.host_context,
                        self.locked_model.clone(),
                        revisions.clone(),
                        ordering.clone(),
                        frames.clone(),
                    );
                    RestoredMachineSnapshot {
                        machine: EvalMachine::for_entry(program.clone(), Vec::new()),
                        contexts: [(Arc::as_ptr(&program.checked) as usize, context)].into(),
                    }
                }
            };
            let waiting_continuation = task
                .waiting_continuation
                .as_ref()
                .map(|resume| {
                    let snapshot = task
                        .machine
                        .as_ref()
                        .ok_or("waiting task has no machine snapshot")?;
                    let mut continuation = |index| {
                        machine.restore_auxiliary(
                            snapshot,
                            index,
                            &root.program,
                            revisions.as_deref(),
                        )
                    };
                    Ok::<_, String>(match resume {
                        TaskResumeSnapshot::Await(index) => {
                            TaskResume::Await(continuation(*index)?)
                        }
                        TaskResumeSnapshot::Run(index) => TaskResume::Run(continuation(*index)?),
                        TaskResumeSnapshot::Transfer {
                            token,
                            continuation: index,
                            output,
                            span,
                        } => TaskResume::Transfer {
                            token: *token,
                            continuation: continuation(*index)?,
                            output: *output,
                            span: *span,
                        },
                    })
                })
                .transpose()?;
            let outcome = task
                .outcome
                .as_ref()
                .map(|outcome| {
                    Ok::<_, String>(match outcome {
                        TaskOutcomeSnapshot::Completed(value) => TaskOutcome::Completed(
                            value.clone().restore_with(machine.active_context())?,
                        ),
                        TaskOutcomeSnapshot::Raised {
                            error_type,
                            payload,
                            span,
                        } => TaskOutcome::Raised(Box::new(PendingPerform::typed_error(
                            *error_type,
                            payload.clone().restore_with(machine.active_context())?,
                            *span,
                            Continuation::BlockValue,
                        ))),
                        TaskOutcomeSnapshot::Failed(failure) => {
                            TaskOutcome::Failed(failure.clone())
                        }
                        TaskOutcomeSnapshot::Cancelled(_) => TaskOutcome::Cancelled(
                            local
                                .cancellation
                                .as_ref()
                                .map_err(ToString::to_string)?
                                .cause()
                                .map_err(|error| error.to_string())?
                                .ok_or(
                                    "cancelled checkpoint task has no restored cancellation cause",
                                )?,
                        ),
                    })
                })
                .transpose()?;
            let state = match task.state {
                ScheduledTaskStateSnapshot::Ready => TaskState::Ready,
                ScheduledTaskStateSnapshot::WaitingTask(id) => {
                    TaskState::Waiting(TaskWait::Task(id))
                }
                ScheduledTaskStateSnapshot::WaitingScope(scope) => {
                    TaskState::Waiting(TaskWait::Scope(scope))
                }
                ScheduledTaskStateSnapshot::WaitingResource(id) => {
                    TaskState::Waiting(TaskWait::Resource(ResourceGuardId(id)))
                }
                ScheduledTaskStateSnapshot::Completed => TaskState::Completed,
                ScheduledTaskStateSnapshot::Raised => TaskState::Raised,
                ScheduledTaskStateSnapshot::Failed => TaskState::Failed,
                ScheduledTaskStateSnapshot::Cancelled => TaskState::Cancelled,
            };
            let cpu_capsule = if task.cpu_pending {
                if state != TaskState::Ready {
                    return Err("pending CPU task is not ready".into());
                }
                let call = machine
                    .machine
                    .pending_cpu_call()
                    .ok_or("pending CPU task lost its pure call entry")?;
                let mut child =
                    self.scheduled_task_context(&program, &mut local, *id, task.handler_floor);
                child.cpu_task_capsule(&call.target, &call.args, call.span)
            } else {
                None
            };
            tasks.insert(
                *id,
                ScheduledTask {
                    scope: task.scope,
                    machine: EvalMachine::new(),
                    local,
                    program: Some(program),
                    handler_floor: task.handler_floor,
                    state,
                    outcome,
                    waiters: task.waiters.iter().copied().map(TaskWaiter::Task).collect(),
                    waiting_continuation,
                    source_index: task.source_index,
                    cpu_capsule,
                    active_slot: task.active_slot,
                    resource_keys: task.resource_keys.clone(),
                },
            );
            restored.insert(*id, machine);
        }
        let scheduler = TaskScheduler::restore_metadata(
            saved.metadata.clone(),
            tasks,
            |origin, program, index| {
                let program = program
                    .map(|program| {
                        CheckedExecutionProgram::restore_identity(
                            program,
                            &root.program,
                            revisions.as_deref(),
                        )
                    })
                    .transpose()?;
                let continuation = index
                    .map(|index| match origin {
                        TaskIdentity::Root => root_restored.restore_auxiliary(
                            root_snapshot,
                            index,
                            &root.program,
                            revisions.as_deref(),
                        ),
                        TaskIdentity::Scheduled(id) => {
                            let snapshot = saved
                                .tasks
                                .get(&id)
                                .and_then(|task| task.machine.as_ref())
                                .ok_or("continuation origin has no live task snapshot")?;
                            restored
                                .get_mut(&id)
                                .ok_or("continuation origin task was not restored")?
                                .restore_auxiliary(
                                    snapshot,
                                    index,
                                    &root.program,
                                    revisions.as_deref(),
                                )
                        }
                    })
                    .transpose()?;
                Ok((continuation, program))
            },
        )?;
        *self.scheduler = scheduler;
        // The scheduler owns machines only after its continuation arena and all
        // shared frame references have been restored successfully.
        for (id, machine) in restored {
            self.scheduler.install_restored_machine(id, machine.machine);
        }
        let counters = &saved.counters;
        self.next_step = self.next_step.max(counters.step);
        self.next_retry = self.next_retry.max(counters.retry);
        self.next_handler_scope = self.next_handler_scope.max(counters.handler_scope);
        self.next_host_request = self.next_host_request.max(counters.host_request);
        self.next_message = self.next_message.max(counters.message);
        self.next_cpu_dispatch = self.next_cpu_dispatch.max(counters.cpu_dispatch);
        self.next_cpu_completion = self.next_cpu_completion.max(counters.cpu_completion);
        self.diagnostics.extend(saved.diagnostics.clone());
        Ok(root_restored.machine)
    }
}

fn restore_machine(
    snapshot: &crate::orchestration::MachineSnapshot,
    program: &CheckedExecutionProgram,
    args: Vec<crate::value::InterpValue>,
    local: &mut super::TaskLocalState,
    identity: TaskIdentity,
    scheduler: &crate::driver::scheduler::TaskSchedulerMetadataSnapshot,
    current: &crate::api::HostExecutionContext,
    locked_model: Option<etas_host::ModelName>,
    limits: &etas_host::StorageLimits,
    revisions: Option<Arc<crate::api::code::PreparedRevisionSet>>,
    ordering: Option<super::machine::snapshot::ordering::OrderedRestoreChecks>,
    frames: super::machine::snapshot::SharedFrameRestore,
) -> Result<super::machine::snapshot::RestoredMachineSnapshot, String> {
    use super::machine::snapshot::{
        ResidualRestoreInput, SnapshotRestoreResources, TaskSnapshotRestoreResources,
    };
    let restored = EvalMachine::from_snapshot_with_contexts(
        snapshot,
        &program.checked,
        program.plan.slots.clone(),
        &program.plan.dispatch,
        &program.plan.closures,
        current,
        SnapshotRestoreResources {
            task: Some(TaskSnapshotRestoreResources {
                scheduler,
                identity,
                frames,
            }),
            locked_model,
            limits,
            revisions,
            ordering,
            root: Some(CheckpointRoot {
                program: program.clone(),
                args,
            }),
            residual: Some(ResidualRestoreInput {
                prepared: program.plan.residual.clone(),
                log: local.residual.log.clone(),
                limits: local.residual.limits,
            }),
        },
    )?;
    local.residual.restore_counters((
        snapshot.residual.next_cursor,
        snapshot.residual.next_attempt,
        snapshot.residual.next_invocation,
    ));
    Ok(restored)
}

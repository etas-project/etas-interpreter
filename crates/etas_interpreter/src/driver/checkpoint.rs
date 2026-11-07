use crate::eval::{EvalContext, machine::EvalMachine};

use super::{
    drive::EvaluationOutcome,
    scheduler::{TaskHandle, TaskHostReactor},
    task,
};

/// Pause task admission and commit work which has already left the coordinator.
/// Successful entry leaves the gate closed until capture finishes. CPU jobs are
/// awaited rather than restarted, so their shared step charge is retained once.
pub(super) async fn enter(
    eval: &mut EvalContext<'_>,
    reactor: &mut TaskHostReactor<'_>,
    root: &mut EvalMachine,
) -> Result<(), EvaluationOutcome> {
    eval.scheduler.begin_checkpoint_barrier();
    let result = drain(eval, reactor, root).await;
    if result.is_err() {
        eval.scheduler.end_checkpoint_barrier();
    }
    result
}

pub(super) async fn create(
    eval: &mut EvalContext<'_>,
    reactor: &mut TaskHostReactor<'_>,
    root: &mut EvalMachine,
    label: Option<String>,
    span: etas_core::Span,
) -> Result<(), EvaluationOutcome> {
    enter(eval, reactor, root).await?;
    let result = eval
        .capture_task_checkpoint(root)
        .map_err(|message| {
            crate::control::ExecutionFault::new(
                etas_core::AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                format!("checkpoint cannot serialize evaluation stack: {message}"),
            )
        })
        .and_then(|captured| {
            eval.record_checkpoint(label, captured.root, Some(captured.scheduler))
        });
    eval.scheduler.end_checkpoint_barrier();
    result.map_err(|fault| {
        eval.diagnostics.push(fault.clone().into_diagnostic());
        EvaluationOutcome::Failed(crate::api::RunFailure::ExecutionFault(fault))
    })
}

async fn drain(
    eval: &mut EvalContext<'_>,
    reactor: &mut TaskHostReactor<'_>,
    root: &mut EvalMachine,
) -> Result<(), EvaluationOutcome> {
    loop {
        task::drain_cpu_completions(eval)?;
        if let Some(completed) = reactor.take_root_completed() {
            if let Some(scope) = root.program_scope() {
                let mut scoped = eval.scoped_context(&scope.program, scope.handler_floor);
                task::commit_host_completion(&mut scoped, root, completed)?;
            } else {
                task::commit_host_completion(eval, root, completed)?;
            }
            root.root_wait = None;
        }
        while let Some((id, completed)) = reactor.take_next_completed_task() {
            let handle = TaskHandle {
                owner: eval.scheduler.task_scope(id),
                task: id,
            };
            let mut scheduled = eval.scheduler.take_checkpoint_task(handle);
            let scope = scheduled.machine.program_scope();
            let (program, floor) = match scope {
                Some(scope) => (scope.program, scope.handler_floor),
                None => (
                    scheduled
                        .program
                        .clone()
                        .expect("Host task has a checked program"),
                    scheduled.handler_floor,
                ),
            };
            let mut child = eval.scheduled_task_context(&program, &mut scheduled.local, id, floor);
            let committed =
                task::commit_host_completion(&mut child, &mut scheduled.machine, completed);
            drop(child);
            eval.scheduler.restore_checkpoint_task(handle, scheduled);
            committed?;
        }
        if eval.cpu_jobs.is_empty() && !reactor.has_pending() {
            task::record_cancellation_events(eval);
            return Ok(());
        }
        task::wait_for_cpu_or_host(eval, reactor).await?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::{PlanOptions, RunOptions},
        control::{CallPurity, CallTarget, Continuation, ControlSignal, PendingCall},
        eval::{EvalContextInput, EvalProgramInput, EvalRunState},
        orchestration::WorkflowEvent,
        testing::{FakeHost, project::checked_project},
        value::InterpValue,
    };

    fn initialize<'a>(
        checked: &'a crate::CheckedProject,
        plan: &'a crate::plan::InterpreterPlan,
        options: RunOptions,
    ) -> (EvalProgramInput<'a>, EvalRunState) {
        initialize_after_steps(checked, plan, options, 0)
    }

    fn initialize_after_steps<'a>(
        checked: &'a crate::CheckedProject,
        plan: &'a crate::plan::InterpreterPlan,
        options: RunOptions,
        consumed_steps: u64,
    ) -> (EvalProgramInput<'a>, EvalRunState) {
        EvalContextInput {
            replay_keys: crate::eval::replay::RunReplayKeys::start(&options).unwrap(),
            code_definitions: options.code_definitions,
            syntax_limits: options.syntax_limits,
            checked,
            plan,
            entry_item: checked.entry.unwrap(),
            entry_args: &[],
            storage_limits: options.storage_limits,
            event_observer: None,
            execution: etas_host::execution::ExecutionScope::new_owned(),
            host_context: options.host_context,
            model_policy: options.model_policy,
            execution_limits: options.execution_limits,
            cpu_threads: options.cpu_threads,
            cpu_min_work: options.cpu_min_work,
            consumed_steps,
            current_session: None,
        }
        .into_parts()
    }

    #[tokio::test]
    async fn barrier_drains_admitted_cpu_work_once_without_dispatching_ready_tasks() {
        let checked = checked_project("module app.main; flow main() -> i32 { return 42; }");
        let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
        let options = RunOptions {
            cpu_threads: std::num::NonZeroUsize::new(4).unwrap(),
            cpu_min_work: 0,
            ..Default::default()
        };
        let (program_input, mut state) = initialize(&checked, &plan, options);
        let mut eval = EvalContext::new(program_input, &mut state);
        let program = eval.checked_execution_program();
        let target = CallTarget::FlowItem(checked.entry.unwrap());
        let span = crate::diagnostics::item_span(&checked, checked.entry.unwrap());
        let mut tasks = Vec::new();
        for index in 0..2 {
            let capsule = eval.cpu_task_capsule(&target, &[], span).unwrap();
            let mut machine = EvalMachine::new();
            machine.resume(ControlSignal::pending_call(PendingCall {
                purity: CallPurity::Ordinary,
                site: None,
                target: target.clone(),
                args: Vec::new(),
                span,
                continuation: Continuation::Return,
            }));
            let handle = eval
                .scheduler
                .spawn(eval.task, machine, Some(program.clone()), 0, index)
                .unwrap();
            eval.scheduler.attach_cpu_capsule(handle, capsule).unwrap();
            tasks.push(handle);
        }
        eval.scheduler
            .set_scope_owner(tasks[0].owner, tasks[0])
            .unwrap();
        let host = FakeHost::new(Default::default());
        let mut reactor = TaskHostReactor::new();
        let mut root = EvalMachine::for_entry(program, Vec::new());
        assert!(matches!(
            task::drive_one(&mut eval, &host, &mut reactor).await,
            Ok(true)
        ));
        assert_eq!(eval.cpu_jobs.len(), 1);
        enter(&mut eval, &mut reactor, &mut root).await.unwrap();
        assert!(eval.cpu_jobs.is_empty());
        assert_eq!(eval.scheduler.dispatch_ready(), None);
        assert!(matches!(eval.scheduler.outcome(tasks[0]).unwrap(),
            Some(super::super::scheduler::TaskOutcome::Completed(value)) if *value == InterpValue::i32(42)));
        assert_eq!(
            eval.scheduler.state(tasks[1]).unwrap(),
            super::super::scheduler::TaskState::Ready
        );
        eval.scheduler.end_checkpoint_barrier();
        let first_charge = eval.consumed_steps();
        assert!(first_charge > 0);

        enter(&mut eval, &mut reactor, &mut root).await.unwrap();
        assert_eq!(eval.consumed_steps(), first_charge);
        let captured = eval.capture_task_checkpoint(&root).unwrap();
        let saved_tasks =
            crate::api::codec::checkpoint::task::roundtrip(&captured.scheduler, &checked);
        let saved_root = crate::api::codec::checkpoint::input::roundtrip_machine(
            &captured.root.machine,
            &checked,
        );
        eval.scheduler.end_checkpoint_barrier();
        assert!(matches!(
            task::drive_one(&mut eval, &host, &mut reactor).await,
            Ok(true)
        ));
        enter(&mut eval, &mut reactor, &mut root).await.unwrap();
        eval.scheduler.end_checkpoint_barrier();
        assert_eq!(eval.consumed_steps(), first_charge * 2);
        drop(eval);
        let events = state.shared.events.into_events();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, WorkflowEvent::TaskCpuCompleted { .. }))
                .count(),
            2
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, WorkflowEvent::HostTrace(_)))
        );
        let (input, mut fresh) = initialize_after_steps(
            &checked,
            &plan,
            RunOptions {
                cpu_threads: std::num::NonZeroUsize::new(4).unwrap(),
                cpu_min_work: 0,
                ..Default::default()
            },
            first_charge,
        );
        let mut resumed = EvalContext::new(input, &mut fresh);
        let mut root = resumed
            .restore_task_checkpoint(
                &saved_tasks,
                &saved_root,
                captured.root.root.unwrap(),
                None,
                None,
            )
            .unwrap();
        assert_eq!(resumed.consumed_steps(), first_charge);
        let mut reactor = TaskHostReactor::new();
        assert!(
            task::drive_one(&mut resumed, &host, &mut reactor)
                .await
                .unwrap()
        );
        assert_eq!(
            resumed.cpu_jobs.len(),
            1,
            "unstarted pure task returns to a CPU worker"
        );
        enter(&mut resumed, &mut reactor, &mut root).await.unwrap();
        resumed.scheduler.end_checkpoint_barrier();
        assert_eq!(resumed.consumed_steps(), first_charge * 2);
        assert!(!resumed.scheduler.has_unsettled_tasks());
        drop(resumed);
        let events = fresh.shared.events.into_events();
        let completions = events
            .iter()
            .filter_map(|event| match event {
                WorkflowEvent::TaskCpuCompleted {
                    job,
                    dispatch_sequence,
                    completion_sequence,
                    ..
                } => Some((*job, *dispatch_sequence, *completion_sequence)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            completions,
            vec![(1, 1, 1)],
            "drained work is not replayed and logical CPU IDs keep advancing"
        );
    }

    #[tokio::test]
    async fn barrier_commits_host_work_without_running_the_ready_continuation() {
        use super::super::dispatch::console::{self, ConsoleDispatchStart};
        use crate::control::{ConsoleDecode, PendingConsole};
        use etas_host::console::{ConsoleOperation, ConsoleRequest};
        let checked = checked_project("module app.main; flow main() -> unit { return; }");
        let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
        let (program_input, mut state) = initialize(&checked, &plan, RunOptions::default());
        let mut eval = EvalContext::new(program_input, &mut state);
        let program = eval.checked_execution_program();
        let span = crate::diagnostics::item_span(&checked, checked.entry.unwrap());
        let handle = eval
            .scheduler
            .spawn(eval.task, EvalMachine::new(), Some(program.clone()), 0, 0)
            .unwrap();
        eval.scheduler
            .set_scope_owner(handle.owner, handle)
            .unwrap();
        assert_eq!(eval.scheduler.dispatch_ready(), Some(handle));
        let mut scheduled = eval.scheduler.take_running(handle).unwrap();
        let host = FakeHost::new(crate::host::HostServiceAvailability {
            host_authority: true,
            console: true,
            policy: true,
            ..Default::default()
        });
        let mut child = eval.scheduled_task_context(&program, &mut scheduled.local, handle.task, 0);
        let console = PendingConsole {
            request: ConsoleRequest {
                id: child.next_host_request_id(),
                operation: ConsoleOperation::WriteStdout {
                    text: "once".into(),
                    newline: true,
                },
                authority: child.host_context.authority.clone(),
                trace: child.host_context.trace.clone(),
                budget: child.host_context.budget.clone(),
            },
            decode: ConsoleDecode::Unit,
            span,
            continuation: Continuation::Return,
        };
        let pending = match console::begin(&mut child, &host, console).await {
            ConsoleDispatchStart::Pending(pending) => pending,
            ConsoleDispatchStart::Complete(_) => {
                panic!("Host request was not admitted: {:?}", child.diagnostics)
            }
        };
        drop(child);
        let operation = pending.operation_id();
        eval.scheduler.restore_running(handle, scheduled).unwrap();
        eval.scheduler.wait_for_host(handle, operation).unwrap();
        let mut reactor = TaskHostReactor::new();
        reactor.start_console(super::super::scheduler::HostOwner::Task(handle), pending);
        let mut root = EvalMachine::for_entry(program.clone(), Vec::new());
        enter(&mut eval, &mut reactor, &mut root).await.unwrap();
        assert!(!reactor.has_pending());
        assert!(reactor.take_next_completed_task().is_none());
        assert_eq!(
            eval.scheduler.state(handle).unwrap(),
            super::super::scheduler::TaskState::Ready
        );
        assert_eq!(eval.scheduler.dispatch_ready(), None);
        assert_eq!(eval.consumed_steps(), 0);
        assert_eq!(host.console_call_count(), 1);
        assert_eq!(host.stdout_text(), "once\n");
        let captured = eval.capture_task_checkpoint(&root).unwrap();
        let saved = crate::api::codec::checkpoint::task::roundtrip(&captured.scheduler, &checked);
        let saved_root = crate::api::codec::checkpoint::input::roundtrip_machine(
            &captured.root.machine,
            &checked,
        );
        eval.scheduler.end_checkpoint_barrier();
        drop(eval);
        let (program_input, mut fresh) = initialize(&checked, &plan, RunOptions::default());
        let mut eval = EvalContext::new(program_input, &mut fresh);
        root = eval
            .restore_task_checkpoint(&saved, &saved_root, captured.root.root.unwrap(), None, None)
            .unwrap();

        enter(&mut eval, &mut reactor, &mut root).await.unwrap();
        eval.scheduler.end_checkpoint_barrier();
        assert_eq!(host.console_call_count(), 1);
        assert!(
            task::drive_one(&mut eval, &host, &mut reactor)
                .await
                .unwrap()
        );
        assert_eq!(
            eval.scheduler.state(handle).unwrap(),
            super::super::scheduler::TaskState::Completed
        );
        assert_eq!(host.console_call_count(), 1);
    }

    #[test]
    fn ready_machine_json_roundtrip_keeps_pending_blocks_and_shared_locals() {
        use crate::{
            control::Frame,
            driver::scheduler::TaskIdentity,
            eval::machine::{
                MachinePoll,
                snapshot::{
                    CaptureContext, CheckpointRoot, SharedFrameCapture, SharedFrameRestore,
                    SnapshotRestoreResources, TaskSnapshotRestoreResources,
                },
            },
        };
        let checked = checked_project(
            r#"
module app.main;
import std.task.{run, Scheduler};
flow main() -> i32 {
    var counter: i32 = 41;
    return run((saved_scheduler: Scheduler) => { counter = counter + 1; return counter; });
}
"#,
        );
        let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
        let (program_input, mut state) = initialize(&checked, &plan, RunOptions::default());
        let mut eval = EvalContext::new(program_input, &mut state);
        let program = eval.checked_execution_program();
        let symbol = checked
            .symbols
            .iter()
            .find(|symbol| symbol.name == "counter")
            .unwrap()
            .id;
        let scheduler_symbol = checked
            .symbols
            .iter()
            .find(|symbol| symbol.name == "saved_scheduler")
            .unwrap()
            .id;
        let block = checked
            .hir
            .exprs
            .iter()
            .find_map(|(_, expr)| match expr {
                etas_hir::HirExpr::Lambda {
                    body: etas_hir::HirLambdaBody::Block(block),
                    ..
                } => Some(*block),
                _ => None,
            })
            .unwrap();
        let mut captured = Frame::from_snapshot(vec![
            (symbol, InterpValue::i32(41)),
            (
                scheduler_symbol,
                InterpValue::Scheduler(crate::value::SchedulerHandleValue::new(
                    crate::driver::scheduler::TaskScopeId(0),
                )),
            ),
        ])
        .unwrap();
        let captures = SharedFrameCapture::default();
        let mut saved = Vec::new();
        for _ in 0..2 {
            let mut machine = EvalMachine::for_entry(program.clone(), Vec::new());
            machine.resume(ControlSignal::pending_block(crate::control::PendingBlock {
                block,
                next_stmt_index: 0,
                frame: captured.clone(),
                residual: None,
                continuation: Continuation::BlockValue,
            }));
            let snapshot = machine
                .capture_machine(&mut CaptureContext::for_tasks(&captures), (0, 0, 0))
                .unwrap();
            assert!(snapshot.input.is_some());
            saved.push(crate::api::codec::checkpoint::input::roundtrip_machine(
                &snapshot, &checked,
            ));
        }
        captured.insert(symbol, InterpValue::i32(99));
        eval.scheduler.begin_checkpoint_barrier();
        let scheduler = eval.scheduler.capture_metadata(Default::default()).unwrap();
        eval.scheduler.end_checkpoint_barrier();
        let frames = SharedFrameRestore::default();
        let limits = eval.storage_limits.clone();
        let mut machines = Vec::new();
        for saved in saved {
            machines.push(
                EvalMachine::from_snapshot_with_resources(
                    &saved,
                    &checked,
                    plan.slots.clone(),
                    &plan.dispatch,
                    &plan.closures,
                    &eval.host_context,
                    SnapshotRestoreResources {
                        task: Some(TaskSnapshotRestoreResources {
                            scheduler: &scheduler,
                            identity: TaskIdentity::Root,
                            frames: frames.clone(),
                        }),
                        locked_model: None,
                        limits: &limits,
                        revisions: None,
                        root: Some(CheckpointRoot {
                            program: program.clone(),
                            args: Vec::new(),
                        }),
                        ordering: None,
                        residual: None,
                    },
                )
                .unwrap(),
            );
        }
        assert_eq!(eval.consumed_steps(), 0);
        let mut first_charge = 0;
        for (index, mut machine) in machines.into_iter().enumerate() {
            loop {
                match machine.run_until_yield(&mut eval) {
                    MachinePoll::Complete(value) => {
                        assert_eq!(*value, InterpValue::i32(42 + index as i32));
                        break;
                    }
                    MachinePoll::CooperativeYield => {}
                    MachinePoll::Fault(fault) => panic!("restored machine: {fault:?}"),
                    _ => panic!("pure restored machine unexpectedly suspended"),
                }
            }
            if index == 0 {
                first_charge = eval.consumed_steps();
                assert!(first_charge > 0);
            } else {
                assert_eq!(eval.consumed_steps(), first_charge * 2);
            }
        }
    }

    #[test]
    fn completed_model_input_roundtrip_keeps_usage_cost_and_tool_payloads() {
        use crate::orchestration::MachineInputSnapshot as I;
        use etas_host::*;
        let checked = checked_project("module app.main; flow main() -> unit { return; }");
        let call = ModelToolCall {
            id: "tool-1".into(),
            tool: "lookup".into(),
            args: HostValue::Record(vec![("key".into(), HostValue::String("a".into()))]),
        };
        let response = ModelResponse {
            id: HostRequestId(7),
            usage: Some(ModelUsage {
                input_tokens: 3,
                output_tokens: 5,
                cost: Some(ModelCostUsage {
                    micros: 17,
                    currency: "USD".into(),
                }),
            }),
            result: Ok(ModelOutput {
                message: ModelMessage {
                    role: ModelRole::Assistant,
                    content: vec![
                        ModelContent::Text("ready".into()),
                        ModelContent::Value(HostValue::Bool(true)),
                    ],
                    tool_call_id: Some("prior".into()),
                    tool_calls: vec![call.clone()],
                },
                tool_calls: vec![call],
            }),
        };
        let input = I::ModelResult(Ok(response.clone()));
        let restored = crate::api::codec::checkpoint::input::roundtrip(&input, &checked);
        let I::ModelResult(restored) = restored else {
            panic!("model response input")
        };
        assert_eq!(restored.unwrap(), response);
    }

    #[tokio::test]
    async fn full_task_json_roundtrip_restores_parent_scopes_waiters_and_root_continuations() {
        use super::super::scheduler::{
            ContinuationTransfer, TaskIdentity, TaskOutcome, TaskResume, TaskState,
        };
        let checked = checked_project(
            "module app.main; flow work() -> i32 { return 42; } flow main() -> i32 { return work(); }",
        );
        let plan = crate::Interpreter.plan(&checked, PlanOptions).plan.unwrap();
        let (input, mut state) = initialize(&checked, &plan, RunOptions::default());
        let eval = EvalContext::new(input, &mut state);
        let program = eval.checked_execution_program();
        let span = crate::diagnostics::item_span(&checked, checked.entry.unwrap());
        let work_symbol = checked
            .symbols
            .iter()
            .find(|symbol| symbol.name == "work")
            .unwrap()
            .id;
        let work = checked
            .hir
            .items
            .iter()
            .find_map(|(id, item)| match item {
                etas_hir::HirItem::Flow(flow) if flow.symbol == work_symbol => Some(id),
                _ => None,
            })
            .unwrap();
        let parent = eval
            .scheduler
            .spawn(eval.task, EvalMachine::new(), Some(program.clone()), 0, 0)
            .unwrap();
        assert_eq!(eval.scheduler.dispatch_ready(), Some(parent));
        let parent_task = eval.scheduler.take_running(parent).unwrap();
        let mut machine = EvalMachine::new();
        machine.resume(ControlSignal::pending_call(PendingCall {
            purity: CallPurity::Ordinary,
            site: None,
            target: CallTarget::FlowItem(work),
            args: Vec::new(),
            span,
            continuation: Continuation::Return,
        }));
        let child = eval
            .scheduler
            .spawn(&parent_task.local, machine, Some(program.clone()), 0, 1)
            .unwrap();
        eval.scheduler.restore_running(parent, parent_task).unwrap();
        eval.scheduler
            .complete(parent, InterpValue::i32(17))
            .unwrap();
        let waiter = eval
            .scheduler
            .spawn(eval.task, EvalMachine::new(), Some(program.clone()), 0, 2)
            .unwrap();
        eval.scheduler
            .set_scope_owner(waiter.owner, waiter)
            .unwrap();
        assert_eq!(eval.scheduler.dispatch_ready(), Some(child));
        eval.scheduler.requeue(child).unwrap();
        assert_eq!(eval.scheduler.dispatch_ready(), Some(waiter));
        let mut waiting = eval.scheduler.take_running(waiter).unwrap();
        waiting.waiting_continuation = Some(TaskResume::Await(Continuation::Return));
        eval.scheduler.restore_running(waiter, waiting).unwrap();
        eval.scheduler.await_task(waiter, child).unwrap();
        let consumed = eval.scheduler.allocate_continuation(
            parent.owner,
            TaskIdentity::Root,
            Continuation::Return,
            etas_types::TypeId(0),
            Some(program.clone()),
            None,
        );
        eval.scheduler
            .consume_continuation(consumed, TaskIdentity::Root, ContinuationTransfer::Requeue)
            .unwrap();
        let retained = eval.scheduler.allocate_continuation(
            parent.owner,
            TaskIdentity::Root,
            Continuation::Return,
            etas_types::TypeId(0),
            Some(program.clone()),
            None,
        );
        let mut root = EvalMachine::for_entry(program.clone(), Vec::new());
        root.resume(ControlSignal::Value(InterpValue::Unit));
        eval.scheduler.begin_checkpoint_barrier();
        let captured = eval.capture_task_checkpoint(&root).unwrap();
        eval.scheduler.end_checkpoint_barrier();
        let snapshot =
            crate::api::codec::checkpoint::task::roundtrip(&captured.scheduler, &checked);
        let root_snapshot = crate::api::codec::checkpoint::input::roundtrip_machine(
            &captured.root.machine,
            &checked,
        );
        assert_eq!(eval.consumed_steps(), 0);

        let (input, mut fresh) = initialize(&checked, &plan, RunOptions::default());
        let mut restored = EvalContext::new(input, &mut fresh);
        let mut root = restored
            .restore_task_checkpoint(
                &snapshot,
                &root_snapshot,
                captured.root.root.unwrap(),
                None,
                None,
            )
            .unwrap();
        assert_eq!(restored.consumed_steps(), 0);
        assert_eq!(
            restored.scheduler.state(parent).unwrap(),
            TaskState::Completed
        );
        assert_eq!(restored.scheduler.state(child).unwrap(), TaskState::Ready);
        assert!(matches!(
            restored.scheduler.consume_continuation(
                consumed,
                TaskIdentity::Root,
                ContinuationTransfer::Resume
            ),
            Err(super::super::scheduler::SchedulerError::ContinuationConsumed)
        ));
        assert!(matches!(
            restored
                .scheduler
                .consume_continuation(retained, TaskIdentity::Root, ContinuationTransfer::Resume)
                .unwrap()
                .continuation,
            Continuation::Return
        ));
        assert!(
            matches!(root.run_until_yield(&mut restored), crate::eval::machine::MachinePoll::Complete(value) if *value == InterpValue::Unit)
        );
        let host = FakeHost::new(Default::default());
        let mut reactor = TaskHostReactor::new();
        for _ in 0..8 {
            if !restored.scheduler.has_unsettled_tasks() {
                break;
            }
            assert!(
                task::drive_one(&mut restored, &host, &mut reactor)
                    .await
                    .unwrap()
            );
        }
        assert!(!restored.scheduler.has_unsettled_tasks());
        for handle in [child, waiter] {
            assert!(
                matches!(restored.scheduler.outcome(handle).unwrap(), Some(TaskOutcome::Completed(value)) if *value == InterpValue::i32(42))
            );
        }
        assert!(
            matches!(restored.scheduler.outcome(parent).unwrap(), Some(TaskOutcome::Completed(value)) if *value == InterpValue::i32(17))
        );
        assert!(restored.consumed_steps() > 0);
    }
}

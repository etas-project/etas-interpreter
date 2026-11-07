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
            consumed_steps: 0,
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
        let mut root = EvalMachine::for_entry(program, Vec::new());
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
        eval.scheduler.end_checkpoint_barrier();

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
}

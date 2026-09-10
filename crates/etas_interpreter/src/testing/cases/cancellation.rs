use super::super::*;
use crate::api::{RunFailure, RunOutcome};
use etas_host::execution::{CancellationReason, ExternalOutcome, ScopeOutcome, StopWait};
use std::time::Duration;

#[tokio::test(flavor = "current_thread")]
async fn stop_crosses_handler_and_retry_without_running_fallback() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.limits.Attempts;
flow main() -> unit ![Console] {
    handle {
        retry limit Attempts(3) {
            println("inflight");
        }
    } with {
        Error<IOError>.raise(_) => { println("caught")?; finish (); }
    };
    println("fallback")?;
}
"#,
    );
    let mut host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let gate = host.pause_console_completion();
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions::default(),
    );
    let control = invocation.control();
    let run = invocation.execute();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(run, async {
            gate.started.notified().await;
            control.stop(CancellationReason::Requested).unwrap();
            gate.release.add_permits(1);
        })
    })
    .await
    .unwrap();
    let result = result.unwrap();
    assert!(matches!(result.outcome, RunOutcome::Cancelled(_)));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.stdout_text(), "inflight\n");
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn cpu_loop_yields_to_external_stop_without_language_error() {
    let checked = checked_project(
        "module app.main; import std.runtime.limits.Iterations; flow main() -> unit { while true limit Iterations(1000000) {} }",
    );
    let host = FakeHost::new(HostServiceAvailability::default());
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions::default(),
    );
    let control = invocation.control();
    let run = invocation.execute();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(run, async {
            tokio::task::yield_now().await;
            control.stop(CancellationReason::Interrupt).unwrap();
        })
    })
    .await
    .expect("CPU computation must yield on a current-thread executor");
    let result = result.unwrap();
    assert!(matches!(result.outcome, RunOutcome::Cancelled(_)));
    assert!(result.value().is_none());
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(matches!(
        control.join().await.unwrap().outcome(),
        ScopeOutcome::Cancelled(_)
    ));
    let json = api::codec::run_report_json("run", &[], "main", &result).unwrap();
    assert_eq!(json["outcome"]["kind"], "cancelled");
    assert_eq!(json["termination"]["local_work_settled"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn stopped_before_first_poll_does_not_call_host() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
flow main() -> unit ![Console, Error<IOError>] { println("must not run"); }
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions::default(),
    );
    let control = invocation.control();
    control.stop(CancellationReason::Requested).unwrap();
    let result = invocation.execute().await;
    let result = result.unwrap();
    assert!(matches!(result.outcome, RunOutcome::Cancelled(_)));
    assert_eq!(host.console_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn stop_wait_preserves_pending_host_completion_and_suppresses_continuation() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
flow main() -> unit ![Console] {
    println("before")?;
    println("after")?;
}
"#,
    );
    let mut host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let gate = host.pause_console_completion();
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions::default(),
    );
    let control = invocation.control();
    let run = invocation.execute();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(run, async {
            gate.started.notified().await;
            control.stop(CancellationReason::Requested).unwrap();
            let StopWait::TimedOut(pending) = control
                .wait_stopped(tokio::time::Instant::now())
                .await
                .unwrap()
            else {
                panic!("stop is not termination");
            };
            assert_eq!(pending.operations().len(), 1);
            assert!(pending.operations()[0].dispatched());
            assert!(!pending.operations()[0].owner_lost());
            gate.release.add_permits(1);
        })
    })
    .await
    .expect("pending work must stay observable until it finishes");
    let result = result.unwrap();
    assert!(matches!(result.outcome, RunOutcome::Cancelled(_)));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.stdout_text(), "before\n");
    assert_eq!(host.console_call_count(), 1);
    let report = result.termination;
    assert_eq!(
        report.operations()[0].outcome(),
        Some(&ExternalOutcome::Confirmed)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_run_retains_lost_host_owner_as_pending_not_terminated() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
flow main() -> unit ![Console, Error<IOError>] { println("once"); }
"#,
    );
    let mut host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let gate = host.pause_console_completion();
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions::default(),
    );
    let control = invocation.control();
    let mut run = Box::pin(invocation.execute());
    tokio::select! {
        _ = gate.started.notified() => {},
        _ = &mut run => panic!("response gate has not been released"),
    }
    drop(run);
    let StopWait::TimedOut(pending) = control
        .wait_stopped(tokio::time::Instant::now())
        .await
        .unwrap()
    else {
        panic!("dropped work is not settled work");
    };
    assert_eq!(pending.operations().len(), 1);
    assert!(pending.operations()[0].owner_lost());
    assert!(pending.operations()[0].outcome().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn late_stop_is_immutable_and_reused_options_create_independent_invocations() {
    let checked = checked_project("module app.main; flow main() -> i32 { return 42; }");
    let host = FakeHost::new(HostServiceAvailability::default());
    let entry = EntryPoint {
        item: checked.entry.unwrap(),
    };
    let options = RunOptions::default();
    let first = Interpreter.create_run(&checked, entry, vec![], &host, options.clone());
    let control = first.control();
    let result = first.execute().await.unwrap();
    assert_eq!(result.value(), Some(&InterpValue::i32(42)));
    control.stop(CancellationReason::Requested).unwrap();
    assert_eq!(
        control.join().await.unwrap().outcome(),
        &ScopeOutcome::Completed
    );
    let second = Interpreter.create_run(&checked, entry, vec![], &host, options);
    let next_control = second.control();
    next_control.stop(CancellationReason::Interrupt).unwrap();
    let result = second.execute().await.unwrap();
    assert!(matches!(result.outcome, RunOutcome::Cancelled(_)));
    assert_eq!(
        control.join().await.unwrap().outcome(),
        &ScopeOutcome::Completed
    );
}

#[tokio::test(flavor = "current_thread")]
async fn unpolled_invocation_and_execution_future_drop_release_body_ownership() {
    let checked = checked_project("module app.main; flow main() -> unit { return; }");
    let host = FakeHost::new(HostServiceAvailability::default());
    for execute in [false, true] {
        let run = Interpreter.create_run(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        );
        let control = run.control();
        assert_eq!(
            control.status().unwrap(),
            etas_host::execution::ScopeState::Running
        );
        assert!(matches!(
            control
                .wait_stopped(tokio::time::Instant::now())
                .await
                .unwrap(),
            StopWait::TimedOut(_)
        ));
        if execute {
            drop(run.execute());
        } else {
            drop(run);
        }
        let report = tokio::time::timeout(Duration::from_secs(1), control.join())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(report.outcome(), ScopeOutcome::Cancelled(cause) if cause.reason() == &CancellationReason::OwnerDropped)
        );
        assert!(report.operations().is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dropped_observers_do_not_cancel_and_waiting_does_not_drive() {
    let checked = checked_project("module app.main; flow main() -> i32 { return 42; }");
    let host = FakeHost::new(HostServiceAvailability::default());
    let run = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions::default(),
    );
    let control = run.control();
    drop(control.clone());
    let mut join = Box::pin(control.join());
    assert!(
        tokio::time::timeout(Duration::from_millis(1), &mut join)
            .await
            .is_err()
    );
    drop(join);
    assert_eq!(
        control.status().unwrap(),
        etas_host::execution::ScopeState::Running
    );
    let result = run.execute().await.unwrap();
    assert_eq!(result.value(), Some(&InterpValue::i32(42)));
    for _ in 0..3 {
        assert_eq!(
            control.join().await.unwrap().outcome(),
            &ScopeOutcome::Completed
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn preparation_failure_is_terminal_and_does_not_dispatch() {
    let checked = checked_project("module app.main; flow main(x: i32) -> i32 { return x; }");
    let host = FakeHost::new(HostServiceAvailability::default());
    let run = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions::default(),
    );
    let control = run.control();
    let result = run.execute().await.unwrap();
    assert!(matches!(
        result.outcome,
        RunOutcome::Failed(RunFailure::PreparationRejected { .. })
    ));
    assert!(!result.diagnostics.is_empty());
    assert!(result.termination.operations().is_empty());
    assert_eq!(
        control.join().await.unwrap().outcome(),
        &ScopeOutcome::Failed
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resume_has_fresh_control_and_unpolled_resume_drop_terminates() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow main() -> string { checkpoint("pause"); return "done"; }
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let options = RunOptions::default();
    let run = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        options.clone(),
    );
    let old = run.control();
    let result = run.execute().await.unwrap();
    let checkpoint = &result.checkpoints[0];
    old.stop(CancellationReason::Requested).unwrap();
    let resume = Interpreter.create_resume(&checked, checkpoint, &host, options.clone());
    let control = resume.control();
    assert_eq!(
        control.status().unwrap(),
        etas_host::execution::ScopeState::Running
    );
    let resumed = resume.execute().await.unwrap();
    assert_eq!(resumed.value(), Some(&InterpValue::String("done".into())));
    assert_eq!(
        old.join().await.unwrap().outcome(),
        &ScopeOutcome::Completed
    );
    let unpolled = Interpreter.create_resume(&checked, checkpoint, &host, options);
    let dropped = unpolled.control();
    drop(unpolled.execute());
    assert!(matches!(
        dropped.join().await.unwrap().outcome(),
        ScopeOutcome::Cancelled(_)
    ));
    assert_eq!(
        control.join().await.unwrap().outcome(),
        &ScopeOutcome::Completed
    );
}

#[test]
fn already_produced_machine_fault_is_not_replaced_by_racing_stop() {
    let checked = checked_project("module app.main; flow main() -> unit { return; }");
    let plan = Interpreter.plan(&checked, api::PlanOptions).plan.unwrap();
    let entry = checked.entry.unwrap();
    let scope = etas_host::execution::ExecutionScope::new_owned();
    let options = RunOptions::default();
    let mut eval = eval::EvalContext::new(eval::EvalContextInput {
        storage_limits: Default::default(),
        event_observer: options.event_observer.clone(),
        execution: scope.clone(),
        checked: &checked,
        plan: &plan,
        host_context: options.host_context,
        model_policy: options.model_policy,
        execution_limits: options.execution_limits,
        consumed_steps: 0,
        current_session: None,
        entry_item: entry,
        entry_args: &[],
    });
    let fault = control::ExecutionFault::new(
        AnalysisDiagnosticCode::UnhandledRuntimeError,
        diagnostics::item_span(&checked, entry),
        "primary failure",
    );
    let mut machine = eval::machine::EvalMachine::new();
    machine.resume(control::ControlSignal::Fault(Box::new(fault.clone())));
    scope
        .cancel_source()
        .stop(CancellationReason::Interrupt)
        .unwrap();
    let eval::machine::MachinePoll::Fault(actual) = machine.run_until_yield(&mut eval) else {
        panic!("an existing fault must remain the primary failure");
    };
    assert_eq!(actual, fault);
}

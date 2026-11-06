use super::super::*;

async fn run(source: &str) -> api::RunResult {
    let checked = checked_project(source);
    Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn structured_task_start_and_await_execute_through_the_scheduler() {
    let result = run(r#"
module app.main;
import std.task.{run, start, await, Scheduler};
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let task = start(() => { return 42; });
        return await(task);
    });
}
"#)
    .await;

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(42)));
}

#[tokio::test(flavor = "current_thread")]
async fn repeated_await_reads_the_cached_terminal_value() {
    let result = run(r#"
module app.main;
import std.task.{run, start, await, Scheduler};
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let task = start(() => { return 20; });
        let first = await(task);
        let second = await(task);
        return first + second;
    });
}
"#)
    .await;

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(40)));
}

#[tokio::test(flavor = "current_thread")]
async fn task_run_settles_unawaited_children_before_returning() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.task.{run, start, Scheduler};
flow main() -> i32 {
    let done = run((scheduler: Scheduler) => {
        let child = start(() => {
            println("child")?;
            return 1;
        });
        return;
    });
    println("after")?;
    return 7;
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(7)));
    assert_eq!(host.stdout_text(), "child\nafter\n");
}

#[tokio::test(flavor = "current_thread")]
async fn child_failure_cancels_a_sibling_waiting_on_host_io() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.task.{run, start, await, Scheduler};
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let blocked = start(() => {
            println("blocked")?;
            return 1;
        });
        let failed = start(() => {
            let one: i32 = 1;
            return one / 0;
        });
        return await(blocked) + await(failed);
    });
}
"#,
    );
    let mut host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let gate = host.pause_console_completion();
    let execution = Interpreter
        .create_run(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .execute();
    tokio::pin!(execution);
    tokio::select! {
        result = &mut execution => panic!(
            "scope returned before its Host operation started cleanup: {:#?}",
            result.unwrap().events
        ),
        () = gate.started.notified() => {}
    }
    gate.release.add_permits(1);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), execution)
        .await
        .expect("cancelled Host I/O cleanup should settle after completion")
        .unwrap();

    assert!(result.value().is_none());
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("numeric division failed")
            && diagnostic.message.contains("divisor is zero")
    }));
    assert_eq!(host.console_call_count(), 1);
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::ScopeExited { outcome, .. } if outcome == "failed"
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn nested_task_runs_publish_owned_scope_lifetimes() {
    let result = run(r#"
module app.main;
import std.task.{run, start, await, Scheduler};
flow main() -> i32 {
    return run((outer: Scheduler) => {
        return run((inner: Scheduler) => {
            let task = start(() => { return 31; });
            return await(task);
        });
    });
}
"#)
    .await;

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(31)));
    let entered = result
        .events
        .iter()
        .filter_map(|event| {
            let WorkflowEvent::ScopeEntered { scope, parent, .. } = event else {
                return None;
            };
            Some((*scope, *parent))
        })
        .collect::<Vec<_>>();
    assert_eq!(entered.len(), 2);
    assert_eq!(entered[1].1, entered[0].0);
    let exited = result
        .events
        .iter()
        .filter_map(|event| {
            let WorkflowEvent::ScopeExited { scope, outcome } = event else {
                return None;
            };
            Some((*scope, outcome.as_str()))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        exited,
        vec![(entered[1].0, "completed"), (entered[0].0, "completed")]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn custom_scheduler_park_resumes_after_task_completion() {
    let result = run(r#"
module app.main;
import std.task.{run, start, await, Scheduler};
import std.task.scheduler.park;
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let task = start(() => { return 7; });
        let resumed = handle { await(task) } with {
            Async.await(waited) as k => {
                park(scheduler, waited, k);
            }
        };
        return resumed + 1;
    });
}
"#)
    .await;

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(8)));
}

#[tokio::test(flavor = "current_thread")]
async fn custom_scheduler_park_observes_an_already_completed_task() {
    let result = run(r#"
module app.main;
import std.task.{run, start, await, Scheduler};
import std.task.scheduler.park;
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let task = start(() => { return 11; });
        let completed = await(task);
        return handle { await(task) } with {
            Async.await(waited) as k => {
                park(scheduler, waited, k);
            }
        };
    });
}
"#)
    .await;

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(11)));
}

#[tokio::test(flavor = "current_thread")]
async fn custom_scheduler_requeue_resumes_with_the_supplied_value() {
    let result = run(r#"
module app.main;
import std.task.{run, start, await, Scheduler};
import std.task.scheduler.requeue;
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        handle { perform Async.yield() } with {
            Async.yield() as k => {
                requeue(scheduler, k, ());
            }
        };
        return 19;
    });
}
"#)
    .await;

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(19)));
}

#[tokio::test(flavor = "current_thread")]
async fn structured_task_trace_records_park_wakeup_and_transfer_order() {
    let result = run(r#"
module app.main;
import std.task.{run, start, await, Scheduler};
import std.task.scheduler.{park, requeue};
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let slow = start(() => {
            handle { perform Async.yield() } with {
                Async.yield() as k => {
                    requeue(scheduler, k, ());
                }
            };
            return 7;
        });
        let waiter = start(() => {
            return handle { await(slow) } with {
                Async.await(waited) as k => {
                    park(scheduler, waited, k);
                }
            };
        });
        return await(waiter);
    });
}
"#)
    .await;

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(7)));

    let (transferred, waiter_task) = result
        .events
        .iter()
        .enumerate()
        .find_map(|(index, event)| {
            let WorkflowEvent::ContinuationTransferred {
                task: Some(task),
                kind,
                ..
            } = event
            else {
                return None;
            };
            (kind == "park").then_some((index, *task))
        })
        .expect("waiter task should transfer its continuation to park");
    let (parked, slow_task) = result
        .events
        .iter()
        .enumerate()
        .find_map(|(index, event)| {
            let WorkflowEvent::TaskParked {
                task, waiting_on, ..
            } = event
            else {
                return None;
            };
            (*task == waiter_task).then_some((index, *waiting_on))
        })
        .expect("waiter task should park on the slow task");
    let ready = result
        .events
        .iter()
        .position(
            |event| matches!(event, WorkflowEvent::TaskReady { task, .. } if *task == waiter_task),
        )
        .expect("waiter task should become ready after the slow task settles");
    let settled = result
        .events
        .iter()
        .position(|event| {
            matches!(
                event,
                WorkflowEvent::TaskSettled {
                    task,
                    outcome,
                    ..
                } if *task == waiter_task && outcome == "completed"
            )
        })
        .expect("waiter task should settle after resumption");
    assert!(parked < transferred && transferred < ready && ready < settled);
    assert_ne!(waiter_task, slow_task);
}

#[tokio::test(flavor = "current_thread")]
async fn a_child_waiting_on_console_does_not_block_its_sibling() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.task.{run, start, await, Scheduler};
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let first = start(() => {
            println("first")?;
            return 1;
        });
        let second = start(() => {
            println("second")?;
            return 2;
        });
        return await(first) + await(second);
    });
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

    let (result, both_started) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(invocation.execute(), async {
            let both_started = tokio::time::timeout(std::time::Duration::from_millis(500), async {
                while host.console_call_count() < 2 {
                    gate.started.notified().await;
                }
            })
            .await
            .is_ok();
            gate.release.add_permits(2);
            both_started
        })
    })
    .await
    .expect("both sibling Host operations should start before either completes");
    let result = result.unwrap();

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert!(
        both_started,
        "expected two concurrent Console calls, observed {}",
        host.console_call_count()
    );
    assert_eq!(result.value(), Some(&InterpValue::i32(3)));
    assert_eq!(host.console_call_count(), 2);
    assert_eq!(host.stdout_text(), "first\nsecond\n");
}

#[tokio::test(flavor = "current_thread")]
async fn the_root_waiting_on_console_keeps_driving_its_children() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.task.{run, start, await, Scheduler};
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let child = start(() => {
            println("child")?;
            return 2;
        });
        println("root")?;
        return await(child) + 1;
    });
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

    let (result, both_started) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(invocation.execute(), async {
            let both_started = tokio::time::timeout(std::time::Duration::from_millis(500), async {
                while host.console_call_count() < 2 {
                    gate.started.notified().await;
                }
            })
            .await
            .is_ok();
            gate.release.add_permits(2);
            both_started
        })
    })
    .await
    .expect("the child Host operation should start while the root operation is pending");
    let result = result.unwrap();

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert!(
        both_started,
        "expected root and child Console calls, observed {}",
        host.console_call_count()
    );
    assert_eq!(result.value(), Some(&InterpValue::i32(3)));
    assert_eq!(host.console_call_count(), 2);
    assert_eq!(host.stdout_text(), "root\nchild\n");
}

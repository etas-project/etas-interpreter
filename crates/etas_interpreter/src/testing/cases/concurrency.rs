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

    let parked = result
        .events
        .iter()
        .position(|event| {
            matches!(
                event,
                WorkflowEvent::TaskParked {
                    task: 1,
                    waiting_on: 0,
                    ..
                }
            )
        })
        .expect("waiter task should park on the slow task");
    let ready = result
        .events
        .iter()
        .position(|event| matches!(event, WorkflowEvent::TaskReady { task: 1, .. }))
        .expect("waiter task should become ready after the slow task settles");
    let settled = result
        .events
        .iter()
        .position(|event| {
            matches!(
                event,
                WorkflowEvent::TaskSettled {
                    task: 1,
                    outcome,
                    ..
                } if outcome == "completed"
            )
        })
        .expect("waiter task should settle after resumption");
    assert!(parked < ready && ready < settled);
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::ContinuationTransferred {
            task: Some(1),
            kind,
            ..
        } if kind == "park"
    )));
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

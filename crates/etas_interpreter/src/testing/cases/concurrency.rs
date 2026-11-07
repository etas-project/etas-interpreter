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

async fn run_delayed_console_demo(
    checked: &etas_frontend::CheckedProject,
    use_concurrency: bool,
    delay: std::time::Duration,
) -> (api::RunResult, std::time::Duration, usize) {
    let mut host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    host.delay_console_completion(delay);
    let started = std::time::Instant::now();
    let result = Interpreter
        .run_checked(
            checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::Bool(use_concurrency)],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    (result, started.elapsed(), host.console_call_count())
}

#[tokio::test(flavor = "current_thread")]
async fn concurrency_speedup_demo() {
    let checked = checked_project(include_str!("../../../examples/concurrency_speedup.es"));
    let operation_delay = std::time::Duration::from_millis(120);

    let (serial, serial_elapsed, serial_calls) =
        run_delayed_console_demo(&checked, false, operation_delay).await;
    let (concurrent, concurrent_elapsed, concurrent_calls) =
        run_delayed_console_demo(&checked, true, operation_delay).await;

    assert!(serial.diagnostics.is_empty(), "{:#?}", serial.diagnostics);
    assert!(
        concurrent.diagnostics.is_empty(),
        "{:#?}",
        concurrent.diagnostics
    );
    assert_eq!(serial.value(), Some(&InterpValue::i32(4)));
    assert_eq!(concurrent.value(), Some(&InterpValue::i32(4)));
    assert_eq!((serial_calls, concurrent_calls), (4, 4));

    let speedup = serial_elapsed.as_secs_f64() / concurrent_elapsed.as_secs_f64();
    println!(
        "concurrency demo: serial={:.1}ms concurrent={:.1}ms speedup={speedup:.2}x",
        serial_elapsed.as_secs_f64() * 1000.0,
        concurrent_elapsed.as_secs_f64() * 1000.0,
    );
    assert!(
        concurrent_elapsed.as_secs_f64() < serial_elapsed.as_secs_f64() / 2.0,
        "expected at least 2x speedup, measured {speedup:.2}x"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn handler_scheduler_speedup_demo() {
    let checked = checked_project(include_str!(
        "../../../examples/handler_scheduler_speedup.es"
    ));
    let operation_delay = std::time::Duration::from_millis(120);

    let (serial, serial_elapsed, serial_calls) =
        run_delayed_console_demo(&checked, false, operation_delay).await;
    let (scheduled, scheduled_elapsed, scheduled_calls) =
        run_delayed_console_demo(&checked, true, operation_delay).await;

    assert!(serial.diagnostics.is_empty(), "{:#?}", serial.diagnostics);
    assert!(
        scheduled.diagnostics.is_empty(),
        "{:#?}",
        scheduled.diagnostics
    );
    assert_eq!(serial.value(), Some(&InterpValue::i32(4)));
    assert_eq!(scheduled.value(), Some(&InterpValue::i32(4)));
    assert_eq!((serial_calls, scheduled_calls), (4, 4));

    let speedup = serial_elapsed.as_secs_f64() / scheduled_elapsed.as_secs_f64();
    println!(
        "handler scheduler demo: serial={:.1}ms handler={:.1}ms speedup={speedup:.2}x",
        serial_elapsed.as_secs_f64() * 1000.0,
        scheduled_elapsed.as_secs_f64() * 1000.0,
    );
    assert!(
        scheduled_elapsed.as_secs_f64() < serial_elapsed.as_secs_f64() / 2.0,
        "expected at least 2x speedup, measured {speedup:.2}x"
    );
    assert_eq!(
        scheduled
            .events
            .iter()
            .filter(|event| matches!(
                event,
                WorkflowEvent::ContinuationConsumed { kind, .. } if kind == "park"
            ))
            .count(),
        4,
        "each await must be scheduled by the Handler"
    );
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
async fn task_join_and_map_concurrent_preserve_input_order() {
    let result = run(r#"
module app.main;
import std.task.{run, join, map_concurrent, Scheduler};
flow main() -> bool {
    return run((scheduler: Scheduler) => {
        let (number, text) = join((
            () => { return 41; },
            () => { return "ok"; }
        ));
        let mapped = map_concurrent([1, 2, 3], (value: i32) => {
            return value * 2;
        });
        let joined = join([() => { return 2; }, () => { return 4; }]);
        return number == 41
            && text == "ok"
            && mapped == [2, 4, 6]
            && joined == [2, 4];
    });
}
"#)
    .await;

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn task_error_combinators_keep_typed_errors_as_data() {
    let result = run(r#"
module app.main;
import std.task.{run, try_join, collect, race, map_concurrent_collect, Scheduler};

flow one() -> i32 ![Error<i32>] {
    if 1 == 0 { return perform Error<i32>.raise(1); }
    return 1;
}
flow fail_seven() -> i32 ![Error<i32>] { return perform Error<i32>.raise(7); }
flow fail_three() -> i32 ![Error<i32>] { return perform Error<i32>.raise(3); }
flow nine() -> i32 ![Error<i32>] {
    if 9 == 0 { return perform Error<i32>.raise(9); }
    return 9;
}
flow race_job(value: i32, fails: bool) -> () -> i32 ![Error<i32>] {
    return () => {
        if fails { return perform Error<i32>.raise(value); }
        return value;
    };
}

flow map_value(value: i32) -> i32 ![Error<i32>] {
    if value < 0 { return perform Error<i32>.raise(0 - value); }
    return value * 10;
}

flow main() -> (Result<Array<i32>, i32>, Array<Result<i32, i32>>, i32, Array<Result<i32, i32>>) ![Error<i32>] {
    return run((scheduler: Scheduler) => {
        let failed = try_join([one, fail_seven]);
        let collected = collect([fail_three, nine]);
        let winner = race<i32, i32>([race_job(5, true), race_job(11, false)]);
        let mapped = map_concurrent_collect<i32, i32, i32>([1, -2, 3], map_value);
        return (failed, collected, winner, mapped);
    });
}
"#)
    .await;

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    let expected = InterpValue::Tuple(
        vec![
            InterpValue::Variant {
                name: "Err".into(),
                fields: vec![InterpValue::i32(7)].into(),
            },
            InterpValue::Array(
                vec![
                    InterpValue::Variant {
                        name: "Err".into(),
                        fields: vec![InterpValue::i32(3)].into(),
                    },
                    InterpValue::Variant {
                        name: "Ok".into(),
                        fields: vec![InterpValue::i32(9)].into(),
                    },
                ]
                .into(),
            ),
            InterpValue::i32(11),
            InterpValue::Array(
                vec![
                    InterpValue::Variant {
                        name: "Ok".into(),
                        fields: vec![InterpValue::i32(10)].into(),
                    },
                    InterpValue::Variant {
                        name: "Err".into(),
                        fields: vec![InterpValue::i32(2)].into(),
                    },
                    InterpValue::Variant {
                        name: "Ok".into(),
                        fields: vec![InterpValue::i32(30)].into(),
                    },
                ]
                .into(),
            ),
        ]
        .into(),
    );
    assert_eq!(result.value(), Some(&expected));
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
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::TaskCancellationRequested { reason, .. } if reason == "ChildFailed"
    )));
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::TaskCleanupCompleted { .. }))
    );
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
    assert_eq!(
        result
            .events
            .iter()
            .filter(|event| matches!(event, WorkflowEvent::ScopePolicy { policy, .. } if policy == "run"))
            .count(),
        2
    );
    assert_eq!(
        result
            .events
            .iter()
            .filter(|event| matches!(event, WorkflowEvent::ScopeResolved { .. }))
            .count(),
        2
    );
    assert_eq!(
        result
            .events
            .iter()
            .filter(|event| matches!(event, WorkflowEvent::ScopeBudgetObserved { .. }))
            .count(),
        2
    );
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

    let consumed = result
        .events
        .iter()
        .position(|event| {
            matches!(
                event,
                WorkflowEvent::ContinuationConsumed { kind, .. } if kind == "park"
            )
        })
        .expect("waiter task should consume its one-shot continuation");
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
    assert!(consumed < parked && parked < transferred && transferred < ready && ready < settled);
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

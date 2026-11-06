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

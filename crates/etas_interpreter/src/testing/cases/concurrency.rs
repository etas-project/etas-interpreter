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

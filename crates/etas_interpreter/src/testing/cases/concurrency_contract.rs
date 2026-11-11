use super::super::*;

async fn execute(
    checked: &etas_frontend::CheckedProject,
    mut options: RunOptions,
    threads: usize,
) -> api::RunResult {
    options.cpu_threads = std::num::NonZeroUsize::new(threads).unwrap();
    options.cpu_min_work = 0;
    Interpreter
        .run_checked(
            checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &FakeHost::new(HostServiceAvailability::default()),
            options,
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn cpu_faults_and_profile_limits_preserve_coordinator_diagnostics() {
    let runtime = api::InterpreterRuntime::new(std::num::NonZeroUsize::new(4).unwrap()).unwrap();
    for (output, expression, steps, depth, bytes, value_depth, expected) in [
        ("i32", "100 / value", None, 512, 1_048_576, 48, "zero"),
        (
            "i32",
            "value",
            Some(2000),
            512,
            1_048_576,
            48,
            "steps (2000)",
        ),
        (
            "i32",
            "recurse(100)",
            None,
            8,
            1_048_576,
            48,
            "call depth (8)",
        ),
        (
            "string",
            "\"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnop\"",
            None,
            512,
            64,
            48,
            "storage limits",
        ),
        (
            "Array<Array<i32>>",
            "[[value]]",
            None,
            512,
            1_048_576,
            1,
            "storage limits",
        ),
    ] {
        let source = format!(
            r#"
module app.main;
import std.task.{{run, map_concurrent, Scheduler}};
flow recurse(n: i32) -> i32 {{
    if n == 0 {{ return 0; }}
    return 1 + recurse(n - 1);
}}
flow job(value: i32) -> {output} {{
    var index = 0;
    while index < 2000 limit Iterations(2000) {{ index = index + 1; }}
    return {expression};
}}
flow main() -> Array<{output}> {{
    return run((scheduler: Scheduler) => {{ return map_concurrent([0], job); }});
}}
"#
        );
        let checked = checked_project(&source);
        let mut options = RunOptions::default();
        options.execution_limits.max_steps = steps.and_then(std::num::NonZeroU64::new);
        options.execution_limits.max_call_depth = std::num::NonZeroU32::new(depth).unwrap();
        options.storage_limits.max_value_bytes = bytes;
        options.storage_limits.max_depth = value_depth;
        let inline = execute(&checked, options.clone(), 1).await;
        options.runtime = Some(runtime.clone());
        let parallel = execute(&checked, options, 4).await;
        assert!(inline.value().is_none(), "{expected}: {:?}", inline.value());
        assert!(
            parallel.value().is_none(),
            "{expected}: {:?}",
            parallel.value()
        );
        assert_eq!(parallel.diagnostics, inline.diagnostics, "{expected}");
        assert!(
            parallel
                .diagnostics
                .iter()
                .any(|d| d.message.contains(expected)),
            "{:#?}",
            parallel.diagnostics
        );
        assert!(parallel.diagnostics.iter().all(|d| d.code
            != etas_core::DiagnosticCode::Analysis(
                etas_core::AnalysisDiagnosticCode::MissingCheckedFact
            )));
        assert_eq!(
            parallel
                .events
                .iter()
                .filter(|event| matches!(event, WorkflowEvent::TaskDispatched { .. }))
                .count(),
            1,
            "{expected}"
        );
        assert_eq!(
            parallel
                .events
                .iter()
                .filter(|event| matches!(event, WorkflowEvent::TaskCpuCompleted { .. }))
                .count(),
            1,
            "{expected}"
        );
    }
}

#[derive(Debug)]
struct BudgetClock {
    start: std::time::Instant,
    expired: std::sync::atomic::AtomicBool,
}

impl etas_host::MonotonicClock for BudgetClock {
    fn now(&self) -> std::time::Instant {
        self.start
            + std::time::Duration::from_millis(
                if self.expired.load(std::sync::atomic::Ordering::Acquire) {
                    2
                } else {
                    0
                },
            )
    }
}

#[derive(Debug)]
struct ExpireTaskBudget {
    clock: std::sync::Arc<BudgetClock>,
    worker: bool,
}

impl api::RunEventObserver for ExpireTaskBudget {
    fn observe(&self, event: &WorkflowEvent) {
        let admitted = if self.worker {
            matches!(event, WorkflowEvent::TaskDispatched { .. })
        } else {
            matches!(event, WorkflowEvent::TaskStarted { task: 1, .. })
        };
        if admitted {
            self.clock
                .expired
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cpu_tasks_use_the_parent_wall_time_ledger() {
    let checked = checked_project(
        r#"
module app.main;
import std.task.{run, map_concurrent, Scheduler};
flow job(value: i32) -> i32 {
    var index = 0;
    while index < 20000 limit Iterations(20000) { index = index + 1; }
    return value;
}

flow main() -> Array<i32> {
    return run((scheduler: Scheduler) => { return map_concurrent([1], job); });
}
"#,
    );
    for threads in [1, 4] {
        let clock = std::sync::Arc::new(BudgetClock {
            start: std::time::Instant::now(),
            expired: std::sync::atomic::AtomicBool::new(false),
        });
        let mut options = RunOptions::default();
        options.host_context.budget = etas_host::ExecutionBudget::start_with_clock(
            Budget {
                time: Some(etas_host::TimeBudget { max_millis: 1 }),
                ..Default::default()
            },
            clock.clone(),
        );
        options.event_observer = Some(std::sync::Arc::new(ExpireTaskBudget {
            clock,
            worker: threads > 1,
        }));
        let result = execute(&checked, options, threads).await;
        assert!(result.value().is_none());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("wall-time budget")),
            "{:?}",
            result.diagnostics
        );
        if threads > 1 {
            assert!(
                result
                    .events
                    .iter()
                    .any(|event| matches!(event, WorkflowEvent::TaskCpuCompleted { .. }))
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cpu_worker_panic_settles_all_admitted_jobs_as_infrastructure_failure() {
    let checked = checked_project(
        r#"
module app.main;
import std.task.{run, map_concurrent, Scheduler};
flow job(value: i32) -> i32 {
    var index = 0;
    while index < 2000 limit Iterations(2000) { index = index + 1; }
    return value;
}
flow main() -> Array<i32> {
    return run((scheduler: Scheduler) => { return map_concurrent([1, 2, 3, 4], job); });
}
"#,
    );
    crate::driver::cpu::panic_next_task();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        execute(&checked, RunOptions::default(), 4),
    )
    .await
    .expect("worker panic must not strand coordinator jobs");
    assert!(
        matches!(&result.outcome, api::RunOutcome::Failed(api::RunFailure::CpuInfrastructure { worker, message })
        if worker.0 < 4 && message.contains("injected CPU task panic")),
        "{:?}",
        result.outcome
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let dispatched = result
        .events
        .iter()
        .filter(|event| matches!(event, WorkflowEvent::TaskDispatched { .. }))
        .count();
    let completed = result
        .events
        .iter()
        .filter(|event| matches!(event, WorkflowEvent::TaskCpuCompleted { .. }))
        .count();
    assert!(dispatched > 0);
    assert_eq!(
        completed, dispatched,
        "every admitted CPU job must be drained"
    );
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::CpuWorkerFailed { .. }))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn one_thousand_short_invocations_release_their_cpu_threads() {
    let checked = checked_project(
        r#"
module app.main;
import std.task.{run, map_concurrent, Scheduler};
flow job(value: i32) -> i32 { return value + 1; }
flow main() -> Array<i32> {
    return run((scheduler: Scheduler) => { return map_concurrent([1, 2], job); });
}
"#,
    );
    crate::driver::cpu::take_test_pools();
    for _ in 0..1000 {
        let result = execute(&checked, RunOptions::default(), 2).await;
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(
            result.value(),
            Some(&InterpValue::Array(
                vec![InterpValue::i32(2), InterpValue::i32(3)].into()
            ))
        );
    }
    let pools = crate::driver::cpu::take_test_pools();
    assert_eq!(pools.len(), 1000);
    assert!(
        pools.iter().all(|pool| pool
            .upgrade()
            .is_none_or(|live| live.load(std::sync::atomic::Ordering::Acquire) == 0)),
        "short invocations must join their workers"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn ten_thousand_map_inputs_share_one_prepared_cpu_program() {
    let checked = checked_project(
        r#"
module app.main;
import std.task.{run, map_concurrent, Scheduler};
flow job(value: i32) -> i32 { return value + 1; }
flow main(values: Array<i32>) -> Array<i32> {
    return run((scheduler: Scheduler) => { return map_concurrent(values, job); });
}
"#,
    );
    let before = crate::driver::cpu::program_build_count();
    let options = RunOptions {
        cpu_threads: std::num::NonZeroUsize::new(4).unwrap(),
        cpu_min_work: 0,
        ..Default::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::Array(
                (0..10000).map(InterpValue::i32).collect::<Vec<_>>().into(),
            )],
            &FakeHost::new(HostServiceAvailability::default()),
            options,
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::Array(
            (1..=10000).map(InterpValue::i32).collect::<Vec<_>>().into()
        ))
    );
    assert_eq!(crate::driver::cpu::program_build_count() - before, 1);
    assert_eq!(
        result
            .events
            .iter()
            .filter(|event| matches!(event, WorkflowEvent::TaskDispatched { .. }))
            .count(),
        10000
    );
}

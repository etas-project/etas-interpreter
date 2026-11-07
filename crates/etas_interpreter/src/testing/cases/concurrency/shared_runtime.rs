use super::*;
use std::{
    collections::HashSet,
    num::NonZeroUsize,
    sync::{Arc, Mutex, atomic::Ordering},
};

#[derive(Debug, Default)]
struct Progress {
    first_dispatch: tokio::sync::Notify,
    sibling_dispatch: tokio::sync::Notify,
    completions: Mutex<Vec<u8>>,
    threads: Mutex<HashSet<std::thread::ThreadId>>,
}

#[derive(Debug)]
struct Probe(u8, Arc<Progress>);
impl api::RunEventObserver for Probe {
    fn observe(&self, event: &WorkflowEvent) {
        match event {
            WorkflowEvent::TaskDispatched { .. } if self.0 == 0 => {
                self.1.first_dispatch.notify_one()
            }
            WorkflowEvent::TaskDispatched { .. } if self.0 == 1 => {
                self.1.sibling_dispatch.notify_one()
            }
            WorkflowEvent::TaskCpuCompleted { .. } => {
                self.1.completions.lock().unwrap().push(self.0)
            }
            _ => {}
        }
    }
    fn observe_cpu(&self, telemetry: &api::CpuTaskTelemetry) {
        self.1
            .threads
            .lock()
            .unwrap()
            .insert(telemetry.thread.unwrap());
    }
}

fn project(iterations: u64) -> etas_frontend::CheckedProject {
    checked_project(&format!(
        r#"
module app.main;
import std.task.{{run, map_concurrent, Scheduler}};
flow job(value: i32) -> i32 {{
    var index = 0;
    while index < {iterations} limit Iterations({iterations}) {{ index = index + 1; }}
    return value + 1;
}}
flow main(values: Array<i32>) -> Array<i32> {{
    return run((scheduler: Scheduler) => {{ return map_concurrent(values, job); }});
}}
"#
    ))
}

fn values(inputs: &[i32]) -> InterpValue {
    InterpValue::Array(
        inputs
            .iter()
            .copied()
            .map(InterpValue::i32)
            .collect::<Vec<_>>()
            .into(),
    )
}

fn options(
    runtime: &api::InterpreterRuntime,
    quota: usize,
    tag: u8,
    progress: &Arc<Progress>,
) -> RunOptions {
    RunOptions {
        runtime: Some(runtime.clone()),
        cpu_threads: NonZeroUsize::new(quota).unwrap(),
        event_observer: Some(Arc::new(Probe(tag, progress.clone()))),
        ..RunOptions::default()
    }
}

fn assert_completed(result: &api::RunResult, inputs: &[i32], quota: usize) {
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&values(
            &inputs.iter().map(|value| value + 1).collect::<Vec<_>>()
        ))
    );
    let mut active = 0;
    let mut peak = 0;
    let mut jobs = Vec::new();
    for event in &result.events {
        match event {
            WorkflowEvent::TaskDispatched { .. } => {
                active += 1;
                peak = peak.max(active);
            }
            WorkflowEvent::TaskCpuCompleted { job, worker, .. } => {
                assert!(worker.0 < 4);
                active -= 1;
                jobs.push(*job);
            }
            _ => {}
        }
    }
    assert_eq!(active, 0);
    assert!(peak > 0 && peak <= quota, "peak={peak}, quota={quota}");
    jobs.sort_unstable();
    assert_eq!(
        jobs,
        (0..inputs.len() as u64).collect::<Vec<_>>(),
        "run-local jobs start at zero"
    );
}

fn assert_pool_released(runtime: api::InterpreterRuntime, progress: &Progress) {
    let threads = progress.threads.lock().unwrap();
    assert!(threads.len() >= 2 && threads.len() <= 4, "{threads:?}");
    let pools = crate::driver::cpu::take_test_pools();
    assert_eq!(pools.len(), 1, "all invocations reuse one OS pool");
    assert_eq!(pools[0].upgrade().unwrap().load(Ordering::Acquire), 4);
    drop(runtime);
    assert!(
        pools[0]
            .upgrade()
            .is_none_or(|live| live.load(Ordering::Acquire) == 0)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn shared_cpu_runtime_bounds_workers_and_fairly_serves_each_run_quota() {
    crate::driver::cpu::take_test_pools();
    let runtime = api::InterpreterRuntime::new(NonZeroUsize::new(4).unwrap()).unwrap();
    let checked = project(20000);
    let progress = Arc::new(Progress::default());
    let host = FakeHost::new(HostServiceAvailability::default());
    let large_inputs = (0..32).collect::<Vec<_>>();
    let small_inputs = [7, 3, 9, 1];
    let large = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![values(&large_inputs)],
        &host,
        options(&runtime, 4, 0, &progress),
    );
    let (large, small) = tokio::join!(large.execute(), async {
        progress.first_dispatch.notified().await;
        Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![values(&small_inputs)],
                &host,
                options(&runtime, 1, 1, &progress),
            )
            .await
    });
    assert_completed(&large.unwrap(), &large_inputs, 4);
    assert_completed(&small.unwrap(), &small_inputs, 1);
    let order = progress.completions.lock().unwrap();
    assert!(
        order.iter().rposition(|run| *run == 1).unwrap()
            < order.iter().rposition(|run| *run == 0).unwrap(),
        "small run completes while the large map still progresses: {order:?}"
    );
    drop(order);
    assert_eq!(host.console_call_count(), 0);
    assert_eq!(host.model_call_count(), 0);
    assert_pool_released(runtime, &progress);
}

#[tokio::test(flavor = "current_thread")]
async fn shared_cpu_runtime_cancels_one_run_and_keeps_siblings_and_pool_usable() {
    crate::driver::cpu::take_test_pools();
    let runtime = api::InterpreterRuntime::new(NonZeroUsize::new(4).unwrap()).unwrap();
    let large_project = project(200000);
    let small_project = project(2000);
    let progress = Arc::new(Progress::default());
    let host = FakeHost::new(HostServiceAvailability::default());
    let inputs = [7, 3, 9, 1];
    let large = Interpreter.create_run(
        &large_project,
        EntryPoint {
            item: large_project.entry.unwrap(),
        },
        vec![values(&(0..32).collect::<Vec<_>>())],
        &host,
        options(&runtime, 4, 0, &progress),
    );
    let control = large.control();
    let (cancelled, sibling, ()) = tokio::join!(
        large.execute(),
        async {
            progress.first_dispatch.notified().await;
            Interpreter
                .run_checked(
                    &small_project,
                    EntryPoint {
                        item: small_project.entry.unwrap(),
                    },
                    vec![values(&inputs)],
                    &host,
                    options(&runtime, 1, 1, &progress),
                )
                .await
                .unwrap()
        },
        async {
            progress.sibling_dispatch.notified().await;
            control
                .stop(etas_host::execution::CancellationReason::Requested)
                .unwrap();
        }
    );
    let cancelled = cancelled.unwrap();
    assert!(matches!(cancelled.outcome, api::RunOutcome::Cancelled(_)));
    assert!(
        cancelled.diagnostics.is_empty(),
        "{:?}",
        cancelled.diagnostics
    );
    assert!(
        cancelled
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::TaskCleanupCompleted { .. }))
    );
    assert_completed(&sibling, &inputs, 1);
    let later = Interpreter
        .run_checked(
            &small_project,
            EntryPoint {
                item: small_project.entry.unwrap(),
            },
            vec![values(&inputs)],
            &host,
            options(&runtime, 8, 2, &progress),
        )
        .await
        .unwrap();
    assert_completed(&later, &inputs, 4);
    assert_pool_released(runtime, &progress);
}

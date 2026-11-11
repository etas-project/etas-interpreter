use super::super::*;

fn memory_tasks(key: &str, inputs: &str) -> etas_frontend::CheckedProject {
    memory_tasks_prepared(key, inputs, "")
}

fn memory_tasks_prepared(key: &str, inputs: &str, prepare: &str) -> etas_frontend::CheckedProject {
    checked_project(&format!(
        r#"
module app.main;
import std.task.{{map_concurrent, run, Scheduler}};
alias Schema = MemoryRegion<{{ Papers: Store<string, string> }}>;
let Memory = std.memory.region<Schema>(stable_id = "resources", store = "test");
flow write(key: string) -> string {{
    {prepare}
    Memory.Papers.put({key}, key);
    return key;
}}
flow main() -> Array<string> {{
    return run((scheduler: Scheduler) => {{ return map_concurrent([{inputs}], write); }});
}}
"#
    ))
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_resource_guards_overlap_distinct_keys_and_wait_for_equal_keys() {
    for (inputs, expected_peak, waits) in [("\"a\", \"b\"", 2, 0), ("\"a\", \"a\"", 1, 1)] {
        let checked = memory_tasks("key", inputs);
        let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
        let probe = host.observe_memory_concurrency(2);
        let mut options = RunOptions::default();
        if waits != 0 {
            options.event_observer =
                Some(std::sync::Arc::new(ReleaseOnResourceWait(probe.clone())));
        }
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Interpreter.run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![],
                &host,
                options,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
        let expected = if waits == 0 { ["a", "b"] } else { ["a", "a"] };
        assert_eq!(
            result.value(),
            Some(&InterpValue::Array(
                expected
                    .into_iter()
                    .map(|v| InterpValue::String(v.into()))
                    .collect::<Vec<_>>()
                    .into()
            ))
        );
        assert_eq!(
            probe.peak.load(std::sync::atomic::Ordering::SeqCst),
            expected_peak,
            "task inputs: {inputs}"
        );
        assert_eq!(host.memory_call_count(), 2);
        assert_eq!(
            result
                .events
                .iter()
                .filter(|e| matches!(e, WorkflowEvent::TaskWaitingResource { .. }))
                .count(),
            waits
        );
        assert_eq!(
            result
                .events
                .iter()
                .filter(|e| matches!(e, WorkflowEvent::TaskResourcesAcquired { permits: 1, .. }))
                .count(),
            2
        );
        assert_eq!(
            result
                .events
                .iter()
                .filter(|e| matches!(e, WorkflowEvent::TaskResourcesReleased { permits: 1, .. }))
                .count(),
            2
        );
        assert!(!result.events.iter().any(|e| matches!(
            e,
            WorkflowEvent::ScopeSerialized { .. } | WorkflowEvent::TaskDispatched { .. }
        )));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_resource_guards_serialize_unresolved_selectors() {
    for (key, prepare) in [("key + \"\"", ""), ("key", "key = \"same\";")] {
        let checked = memory_tasks_prepared(key, "\"a\", \"b\"", prepare);
        let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
        let probe = host.observe_memory_concurrency(1);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Interpreter.run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![],
                &host,
                RunOptions::default(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(probe.peak.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(host.memory_call_count(), 2);
        assert!(result.events.iter().any(|e| matches!(e, WorkflowEvent::ScopeSerialized { reason, .. } if reason == "unresolved_resource_selector")));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_resource_guards_acquire_all_resources_before_reverse_order_jobs() {
    let checked = checked_project(
        r#"
module app.main;
import std.task.{run, join, Scheduler};
alias Schema = MemoryRegion<{ Papers: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "resources", store = "test");
flow forward() -> i32 { Memory.Papers.put("a", "one"); Memory.Papers.put("b", "two"); return 1; }
flow reverse() -> i32 { Memory.Papers.put("b", "three"); Memory.Papers.put("a", "four"); return 2; }
flow main() -> Array<i32> { return run((scheduler: Scheduler) => { return join([forward, reverse]); }); }
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        Interpreter.run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::Array(
            vec![InterpValue::i32(1), InterpValue::i32(2)].into()
        ))
    );
    assert_eq!(host.memory_call_count(), 4);
    assert_eq!(
        result
            .events
            .iter()
            .filter(|e| matches!(e, WorkflowEvent::TaskResourcesAcquired { permits: 2, .. }))
            .count(),
        2
    );
    assert_eq!(
        result
            .events
            .iter()
            .filter(|e| matches!(e, WorkflowEvent::TaskResourcesReleased { permits: 2, .. }))
            .count(),
        2
    );
}

#[derive(Debug)]
struct WaitingResource(std::sync::Arc<tokio::sync::Notify>);

#[derive(Debug)]
struct ReleaseOnResourceWait(std::sync::Arc<crate::testing::host::MemoryConcurrencyProbe>);
impl api::RunEventObserver for ReleaseOnResourceWait {
    fn observe(&self, event: &WorkflowEvent) {
        if matches!(event, WorkflowEvent::TaskWaitingResource { .. }) {
            self.0.release();
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_resource_guards_resolve_captured_keys_through_helper_calls() {
    let checked = checked_project(
        r#"
module app.main;
import std.task.{run, start, await, Scheduler};
alias Schema = MemoryRegion<{ Papers: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "resources", store = "test");
flow write(key: string) -> string { Memory.Papers.put(key, key); return key; }
flow helper(key: string) -> string { return write(key); }
flow main() -> string {
    return run((scheduler: Scheduler) => {
        let left = "a";
        let right = "b";
        let a = start(() => { return helper(left); });
        let b = start(() => { return helper(right); });
        return await(a) + await(b);
    });
}
"#,
    );
    let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    let probe = host.observe_memory_concurrency(2);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        Interpreter.run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::String("ab".into())));
    assert_eq!(probe.peak.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(host.memory_call_count(), 2);
    assert!(
        !result
            .events
            .iter()
            .any(|e| matches!(e, WorkflowEvent::ScopeSerialized { .. }))
    );
}
impl api::RunEventObserver for WaitingResource {
    fn observe(&self, event: &WorkflowEvent) {
        if matches!(event, WorkflowEvent::TaskWaitingResource { .. }) {
            self.0.notify_one();
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_resource_guards_release_permits_when_the_run_is_cancelled() {
    let checked = memory_tasks("key", "\"a\", \"a\"");
    let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    let probe = host.observe_memory_concurrency(2);
    let waiting = std::sync::Arc::new(tokio::sync::Notify::new());
    let options = RunOptions {
        event_observer: Some(std::sync::Arc::new(WaitingResource(waiting.clone()))),
        ..Default::default()
    };
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        options,
    );
    let control = invocation.control();
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(invocation.execute(), async {
            waiting.notified().await;
            control
                .stop(etas_host::execution::CancellationReason::Requested)
                .unwrap();
            probe.release();
        })
    })
    .await
    .unwrap();
    let result = result.unwrap();
    assert!(
        matches!(result.outcome, api::RunOutcome::Cancelled(_)),
        "{:?}",
        result.outcome
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.memory_call_count(), 1);
    assert_eq!(
        result
            .events
            .iter()
            .filter(|e| matches!(e, WorkflowEvent::TaskResourcesAcquired { .. }))
            .count(),
        result
            .events
            .iter()
            .filter(|e| matches!(e, WorkflowEvent::TaskResourcesReleased { .. }))
            .count()
    );
}

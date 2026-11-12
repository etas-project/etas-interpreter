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
async fn runtime_resource_guards_coarsen_dynamic_keys_to_their_store() {
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
        assert!(!result.events.iter().any(|e| matches!(
            e,
            WorkflowEvent::ScopeSerialized { reason, .. }
                if reason == "unresolved_resource_selector"
        )));
        assert_eq!(
            result
                .events
                .iter()
                .filter(|event| matches!(event, WorkflowEvent::TaskWaitingResource { .. }))
                .count(),
            1
        );
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

#[tokio::test(flavor = "current_thread")]
async fn run_owner_memory_guards_allow_disjoint_children_and_block_conflicts() {
    for (child_region, expected_while_console_waits) in [("Peer", 2), ("Owner", 1)] {
        let checked = checked_project(&format!(
            r#"
module app.main;
import std.io.read_line;
import std.task.{{run, start, Scheduler}};
alias Schema = MemoryRegion<{{ Values: Store<string, i32> }}>;
let Owner = std.memory.region<Schema>(stable_id = "owner-records", store = "owner");
let Peer = std.memory.region<Schema>(stable_id = "peer-records", store = "peer");
flow main() -> i32 {{
    return run((scheduler: Scheduler) => {{
        Owner.Values.put("owner", 1);
        let child = start(() => {{
            {child_region}.Values.put("owner", 1);
            return 0;
        }});
        read_line();
        return 0;
    }});
}}
"#,
        ));
        let mut host = FakeHost::new(availability(&[
            HostRequirementKind::DurableMemory,
            HostRequirementKind::Console,
        ]));
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

        let (result, observed_while_waiting) =
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                tokio::join!(invocation.execute(), async {
                    gate.started.notified().await;
                    tokio::time::timeout(std::time::Duration::from_millis(250), async {
                        while host.memory_call_count() < 2 {
                            tokio::task::yield_now().await;
                        }
                    })
                    .await
                    .ok();
                    let observed = host.memory_call_count();
                    gate.release.add_permits(1);
                    observed
                })
            })
            .await
            .expect("the run owner and child should settle after console input is released");
        let result = result.unwrap();

        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::i32(0)));
        assert_eq!(
            observed_while_waiting, expected_while_console_waits,
            "child region: {child_region}"
        );
        assert_eq!(host.memory_call_count(), 2);
        assert!(!result.events.iter().any(|event| matches!(
            event,
            WorkflowEvent::ScopeSerialized { reason, .. }
                if reason == "unresolved_resource_selector"
        )));
        assert_eq!(
            result
                .events
                .iter()
                .filter(|event| matches!(event, WorkflowEvent::TaskWaitingResource { .. }))
                .count(),
            usize::from(child_region == "Owner")
        );
        assert_eq!(
            result
                .events
                .iter()
                .filter(|event| matches!(
                    event,
                    WorkflowEvent::TaskResourcesAcquired { permits: 1, .. }
                ))
                .count(),
            2
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn run_owner_allows_a_disjoint_child_with_dynamic_memory_keys() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.read_line;
import std.task.{run, start, Scheduler};
import std.text.from_i32;
alias Schema = MemoryRegion<{ Values: Store<string, i32> }>;
let Owner = std.memory.region<Schema>(stable_id = "owner-records", store = "owner");
let Peer = std.memory.region<Schema>(stable_id = "peer-records", store = "peer");
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        Owner.Values.put("owner", 1);
        let child = start(() => {
            var index: i32 = 0;
            while index < 2 limit Iterations(2) {
                Peer.Values.put("part-" + from_i32(index), index);
                index = index + 1;
            }
            return 0;
        });
        read_line();
        return 0;
    });
}
"#,
    );
    let mut host = FakeHost::new(availability(&[
        HostRequirementKind::DurableMemory,
        HostRequirementKind::Console,
    ]));
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

    let (result, writes_while_waiting) =
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(invocation.execute(), async {
                gate.started.notified().await;
                let progressed =
                    tokio::time::timeout(std::time::Duration::from_millis(500), async {
                        while host.memory_call_count() < 3 {
                            tokio::task::yield_now().await;
                        }
                    })
                    .await
                    .is_ok();
                gate.release.add_permits(1);
                progressed
            })
        })
        .await
        .expect("the dynamic collector should progress while console input is pending");
    let result = result.unwrap();

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert!(writes_while_waiting);
    assert_eq!(result.value(), Some(&InterpValue::i32(0)));
    assert_eq!(host.memory_call_count(), 3);
    assert!(!result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::ScopeSerialized { reason, .. }
            if reason == "unresolved_resource_selector"
    )));
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::TaskWaitingResource { .. }))
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
    let mut options = RunOptions::default();
    options.event_observer = Some(std::sync::Arc::new(WaitingResource(waiting.clone())));
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

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn concurrent_checkpoint_restores_resource_holder_and_waiter() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
import std.task.{run, start, await, Scheduler};
alias Schema = MemoryRegion<{ Papers: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "checkpoint-resources", store = "test");
flow write(key: string) -> string {
    Memory.Papers.put(key, key);
    var step = 0;
    while step < 10000 limit Iterations(10000) { step = step + 1; }
    return key;
}
flow main() -> Array<string> {
    return run((scheduler: Scheduler) => {
        let first = start(() => { return write("a"); });
        let second = start(() => { return write("a"); });
        let marker = start(() => {
            var step = 0;
            while step < 1000 limit Iterations(1000) { step = step + 1; }
            checkpoint("resources");
            return ();
        });
        let a = await(first);
        let b = await(second);
        await(marker);
        return [a, b];
    });
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Checkpoint,
        HostRequirementKind::DurableMemory,
    ]));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(host.memory_call_count(), 2);
    let checkpoint = &first.checkpoints[0];
    assert!(
        checkpoint
            .tasks
            .as_ref()
            .unwrap()
            .tasks
            .values()
            .any(|task| matches!(
                task.state,
                crate::orchestration::ScheduledTaskStateSnapshot::WaitingResource(_)
            )),
        "resource waiter must survive the barrier"
    );
    let artifact =
        crate::api::codec::checkpoint_artifact_json(&["main.es".into()], "main", checkpoint)
            .unwrap();
    let bytes = serde_json::to_vec(&*artifact).unwrap();
    let artifact = serde_json::from_slice(&bytes).unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), first.value());
    assert_eq!(
        host.memory_call_count(),
        3,
        "only the waiting write runs after restore"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_checkpoint_keeps_completed_host_error_for_its_handler() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.read_all;
import std.runtime.checkpoint;
import std.task.{run, start, await, Scheduler};
flow main() -> string {
    return run((scheduler: Scheduler) => {
        let marker = start(() => { checkpoint("io-error"); return (); });
        let recovered = handle { read_all() } with {
            Error<IOError>.raise(_) => { finish "recovered"; }
        };
        await(marker);
        return recovered;
    });
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Checkpoint,
        HostRequirementKind::Console,
    ]));
    host.fail_console(HostErrorCode::ProviderUnavailable, "stdin closed");
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(
        first.value(),
        Some(&InterpValue::String("recovered".into()))
    );
    assert_eq!(host.console_call_count(), 1);
    let saved = &first.checkpoints[0];
    assert!(
        saved
            .tasks
            .as_ref()
            .unwrap()
            .tasks
            .values()
            .any(|task| matches!(
                task.machine
                    .as_ref()
                    .and_then(|machine| machine.input.as_deref())
                    .and_then(crate::orchestration::MachineInputSnapshot::signal),
                Some(crate::orchestration::ControlSignalSnapshot::Error { .. })
            )),
        "Host error is committed before its handler runs"
    );
    let wire =
        crate::api::codec::checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap();
    let bytes = serde_json::to_vec(&*wire).unwrap();
    let wire = serde_json::from_slice(&bytes).unwrap();
    let saved = crate::api::codec::checkpoint_from_json(&wire, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &saved, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), first.value());
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_checkpoint_restores_child_request_and_handler_park_from_json() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
import std.task.{run, start, await, Scheduler};
import std.task.scheduler.park;
flow main() -> i32 {
    return run((scheduler: Scheduler) => {
        let task = start(() => { checkpoint("child"); return 7; });
        let other = start(() => { return 1; });
        let resumed = handle { await(task) } with {
            Async.await(waited) as k => { park(scheduler, waited, k); }
        };
        return resumed + await(other);
    });
}


"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.value(), Some(&InterpValue::i32(8)));
    assert_eq!(first.checkpoints.len(), 1);
    let wire = crate::api::codec::checkpoint_artifact_json(
        &["main.es".into()],
        "main",
        &first.checkpoints[0],
    )
    .unwrap();
    let bytes = serde_json::to_vec(&*wire).unwrap();
    let json = serde_json::from_slice(&bytes).unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&json, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), first.value());
    assert!(
        resumed.checkpoints.is_empty(),
        "the child resumes after its saved checkpoint"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_checkpoint_drains_cpu_and_model_without_replaying_or_recharging() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
import std.task.{run, start, await, Scheduler};
agent Solver() -> string { return perform infer(Prompt.new().data("hello")); }
flow burn() -> i32 {
    var index = 0;
    while index < 2000 limit Iterations(2000) { index = index + 1; }
    return index;
}
flow main() -> string {
    return run((scheduler: Scheduler) => {
        let cpu = start(burn);
        let marker = start(() => { checkpoint("while-model"); return 9; });
        let answer = Solver.run();
        await(cpu);
        await(marker);
        return answer;
    });
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Checkpoint,
        HostRequirementKind::Agentic,
    ]));
    host.seed_model_response_text_with_usage(
        "ok",
        Some(etas_host::ModelUsage {
            input_tokens: Some(2),
            output_tokens: Some(3),
            cost: None,
        }),
    );
    let runtime = api::InterpreterRuntime::new(std::num::NonZeroUsize::new(4).unwrap()).unwrap();
    let options = || {
        let mut options = RunOptions {
            runtime: Some(runtime.clone()),
            cpu_threads: std::num::NonZeroUsize::new(4).unwrap(),
            cpu_min_work: 0,
            ..Default::default()
        };
        options
            .host_context
            .authority
            .grants
            .push(HostActionGrant::allow("Agentic", "infer"));
        options.host_context.budget = etas_host::ExecutionBudget::start(Budget {
            tokens: Some(TokenBudget { max_tokens: 5 }),
            ..Default::default()
        });
        options
    };
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            options(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.value(), Some(&InterpValue::String("ok".into())));
    assert_eq!(host.model_call_count(), 1);
    assert!(
        first
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::TaskCpuCompleted { .. }))
    );
    let saved = first.checkpoints.first().expect("child checkpoint");
    let tasks = saved.tasks.as_ref().unwrap();
    assert!(
        tasks.tasks.values().any(|task| matches!(
            task.machine
                .as_ref()
                .and_then(|machine| machine.input.as_deref()),
            Some(crate::orchestration::MachineInputSnapshot::ModelResult(_))
        )),
        "model completion is committed at the barrier"
    );
    let wire =
        crate::api::codec::checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap();
    let bytes = serde_json::to_vec(&*wire).unwrap();
    let wire = serde_json::from_slice(&bytes).unwrap();
    let saved = crate::api::codec::checkpoint_from_json(&wire, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &saved, &host, options())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), first.value());
    assert_eq!(host.model_call_count(), 1);
    assert!(
        !resumed
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::TaskDispatched { .. })),
        "drained CPU work remains cached"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_checkpoint_preserves_completed_scope_owner_and_nested_waits() {
    for (kind, body, expected) in [
        (
            "await",
            r#"let child = start(() => { checkpoint("nested-await"); return "ready"; }); return await(child);"#,
            "ready",
        ),
        (
            "completed-owner",
            r#"start(() => { checkpoint("returned-owner"); return "ignored"; }); return "owner-result";"#,
            "owner-result",
        ),
        (
            "inherited-handler",
            r#"return handle { let child = start(() => { checkpoint("inherited-handler"); return "ready"; }); await(child); return perform Pulse.tick(); } with { Pulse.tick() => { resume "handled"; } };"#,
            "handled",
        ),
        (
            "inherited-retry",
            r#"retry limit Attempts(1) { let child = start(() => { checkpoint("inherited-retry"); return "retried"; }); return await(child); } return "failed";"#,
            "retried",
        ),
    ] {
        let checked = checked_project(&format!(
            r#"
module app.main;
import std.runtime.checkpoint;
import std.task.{{run, start, await, Scheduler}};
import std.runtime.limits.Attempts;
effect Pulse {{ action tick() -> string; }}
flow main() -> string {{ return run((scheduler: Scheduler) => {{ {body} }}); }}
"#
        ));
        let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
        let first = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                Vec::new(),
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(
            first.diagnostics.is_empty(),
            "{kind}: {:?}",
            first.diagnostics
        );
        assert_eq!(
            first.value(),
            Some(&InterpValue::String(expected.into())),
            "{kind}"
        );
        let saved = first.checkpoints.first().expect(kind);
        if kind == "completed-owner" {
            assert!(
                saved
                    .tasks
                    .as_ref()
                    .unwrap()
                    .tasks
                    .values()
                    .any(|task| matches!(
                        task.state,
                        crate::orchestration::ScheduledTaskStateSnapshot::Completed
                    )),
                "the owner returned before its child requested checkpoint"
            );
        }
        let wire = crate::api::codec::checkpoint_artifact_json(&["main.es".into()], "main", saved)
            .unwrap();
        let bytes = serde_json::to_vec(&*wire).unwrap();
        let wire = serde_json::from_slice(&bytes).unwrap();
        let saved = crate::api::codec::checkpoint_from_json(&wire, &checked).unwrap();
        let resumed = Interpreter
            .resume_checkpoint(&checked, &saved, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(
            resumed.diagnostics.is_empty(),
            "{kind}: {:?}",
            resumed.diagnostics
        );
        assert_eq!(resumed.value(), first.value(), "{kind}");
        assert!(
            resumed.checkpoints.is_empty(),
            "{kind}: child resumes after checkpoint"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_checkpoint_drains_native_kernel_and_preserves_step_accounting() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
import std.text.uppercase;
import std.task.{run, start, await, Scheduler};
flow main(input: string) -> string {
    return run((scheduler: Scheduler) => {
        let native = start(() => { return uppercase(input); });
        let marker = start(() => { checkpoint("native"); return (); });
        let value = await(native);
        await(marker);
        checkpoint("after");
        return value;
    });
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![InterpValue::String("ab".repeat(100000).into())],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(
        first.value(),
        Some(&InterpValue::String("AB".repeat(100000).into()))
    );
    assert_eq!(first.checkpoints.len(), 2);
    let before = &first.checkpoints[0];
    assert!(before.tasks.as_ref().unwrap().tasks.values().any(|task|
        matches!(task.machine.as_ref().and_then(|machine| machine.input.as_deref()).and_then(crate::orchestration::MachineInputSnapshot::signal),
            Some(crate::orchestration::ControlSignalSnapshot::Input(
                crate::orchestration::ContinuationInputSnapshot::Value(crate::orchestration::ValueSnapshot::String(value)))) if value.len() == 200000)),
        "checkpoint drains the admitted kernel before capturing the ready input");
    let wire =
        crate::api::codec::checkpoint_artifact_json(&["main.es".into()], "main", before).unwrap();
    let bytes = serde_json::to_vec(&*wire).unwrap();
    let wire = serde_json::from_slice(&bytes).unwrap();
    let saved = crate::api::codec::checkpoint_from_json(&wire, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &saved, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), first.value());
    assert_eq!(resumed.checkpoints.len(), 1);
    assert_eq!(
        resumed.checkpoints[0].execution_progress.consumed_steps,
        first.checkpoints[1].execution_progress.consumed_steps,
        "native chunks are charged once across restore"
    );
}

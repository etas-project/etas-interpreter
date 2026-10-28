use super::super::*;

async fn assert_true(source: &str) {
    let checked = checked_project(source);
    let host = FakeHost::new(availability(&[]));
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
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)), "{source}");
}

#[tokio::test(flavor = "current_thread")]
async fn priority_queue_uses_priority_not_insertion_order() {
    for entries in [
        [(10, 1), (20, 3), (30, 2)],
        [(30, 2), (20, 3), (10, 1)],
        [(20, 3), (10, 1), (30, 2)],
        [(20, 3), (30, 2), (10, 1)],
        [(10, 1), (30, 2), (20, 3)],
        [(30, 2), (10, 1), (20, 3)],
    ] {
        let pushes = entries
            .iter()
            .map(|(value, priority)| format!(".push({value}, {priority})"))
            .collect::<String>();
        assert_true(&format!(
            r#"
module app.main;
flow main() -> bool {{
    let queue = PriorityQueue.new<i32, i32>(){pushes};
    let alias = queue;
    let (tail1, first) = queue.pop();
    let (tail2, second) = tail1.pop();
    let (tail3, third) = tail2.pop();
    let (_, empty) = tail3.pop();
    let (_, original) = alias.pop();
    return first == Some(20) && second == Some(30) && third == Some(10)
        && empty == None && original == first;
}}
"#
        ))
        .await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn priority_queue_equal_priorities_are_fifo() {
    assert_true(
        r#"
module app.main;
flow main() -> bool {
    let queue = PriorityQueue.new<string, i32>()
        .push("first", 2).push("highest", 3).push("second", 2).push("third", 2);
    let (tail1, highest) = queue.pop();
    let (tail2, first) = tail1.pop();
    let (tail3, second) = tail2.pop();
    let (_, third) = tail3.pop();
    return highest == Some("highest") && first == Some("first")
        && second == Some("second") && third == Some("third");
}
"#,
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn priority_queue_custom_comparison_can_reverse_the_default_order() {
    assert_true(
        r#"
module app.main;
flow smallest_first(a: i32, b: i32) -> i32 ![] {
    if a < b { return 1; }
    if a > b { return -1; }
    return 0;
}
flow main() -> bool {
    let queue = PriorityQueue.new<string, i32>(smallest_first)
        .push("high", 9).push("low", 1).push("middle", 5);
    let (tail, first) = queue.pop();
    let (_, second) = tail.pop();
    return first == Some("low") && second == Some("middle");
}
"#,
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn priority_queue_comparison_supports_user_nominal_priority_types() {
    assert_true(
        r#"
module app.main;
type Rank = { urgent: bool, deadline: i32 };
flow compare(a: Rank, b: Rank) -> i32 ![] {
    if a.urgent && !b.urgent { return 1; }
    if b.urgent && !a.urgent { return -1; }
    if a.deadline < b.deadline { return 1; }
    if a.deadline > b.deadline { return -1; }
    return 0;
}
flow main() -> bool {
    let queue = PriorityQueue.new<string, Rank>(compare)
        .push("later", Rank { urgent = false, deadline = 0 })
        .push("first", Rank { urgent = true, deadline = 2 })
        .push("second", Rank { urgent = true, deadline = 2 });
    let (tail, first) = queue.pop();
    let (_, second) = tail.pop();
    return first == Some("first") && second == Some("second");
}
"#,
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn priority_queue_keeps_argument_effect_order_and_captured_comparison() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
flow comparison() -> (i32, i32) -> i32 ![] {
    println("comparison")?;
    let direction = -1;
    return (a: i32, b: i32) => {
        if a < b { return 0 - direction; }
        if a > b { return direction; }
        return 0;
    };
}
flow value() -> string { println("value")?; return "small"; }
flow priority() -> i32 { println("priority")?; return 1; }
flow main() -> bool {
    let queue = PriorityQueue.new<string, i32>(comparison())
        .push("large", 9).push(value(), priority());
    let (_, first) = queue.pop();
    return first == Some("small");
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
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(host.stdout_text(), "comparison\nvalue\npriority\n");
    assert_eq!(host.console_call_count(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn priority_queue_rejects_default_handled_actions_in_a_comparison() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
flow compare(a: i32, b: i32) -> i32 ![] {
    println("must not run")?;
    return 0;
}
flow main() -> unit {
    let queue = PriorityQueue.new<string, i32>(compare);
    return;
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
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("comparison must be pure")),
        "{:?}",
        result.diagnostics
    );
    assert_eq!(host.console_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn priority_queue_comparison_consumes_execution_fuel() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.limits.Iterations;
flow compare(a: i32, b: i32) -> i32 ![] { while true limit Iterations(10000000) {} return 0; }
flow main() -> unit {
    let queue = PriorityQueue.new<string, i32>(compare).push("a", 1).push("b", 2);
    return;
}
"#,
    );
    let host = FakeHost::new(availability(&[]));
    let mut options = RunOptions::default();
    options.execution_limits.max_steps = std::num::NonZeroU64::new(256);
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
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("step")),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn priority_queue_checkpoints_preserve_comparison_fifo_and_old_aliases() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
import std.io.println;
flow wrong_parameters(a: string, b: string) -> i32 ![] { return 0; }
flow wrong_result(a: i32, b: i32) -> string ![] { return "wrong"; }
flow loud(a: i32, b: i32) -> i32 ![] { println("must not run")?; return 0; }
flow compare(a: i32, b: i32) -> i32 ![] {
    if a < b { return 1; }
    if a > b { return -1; }
    return 0;
}
flow main() -> bool {
    let queue = PriorityQueue.new<string, i32>(compare)
        .push("first", 2).push("high", 9).push("second", 2).push("low", 1);
    let original = queue;
    checkpoint("stable-priority");
    let (tail1, low) = queue.pop();
    let (tail2, first) = tail1.pop();
    let (tail3, second) = tail2.pop();
    let (empty, high) = tail3.pop();
    let (_, missing) = empty.pop();
    let (_, old) = original.pop();
    return low == Some("low") && first == Some("first") && second == Some("second")
        && high == Some("high") && missing == None && old == low;
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
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
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    let checkpoint = &result.checkpoints[0];
    let artifact =
        crate::api::codec::checkpoint_artifact_json(&["main.es".into()], "main", checkpoint)
            .unwrap();
    let decoded = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    for snapshot in [checkpoint, &decoded] {
        let result = Interpreter
            .resume_checkpoint(&checked, snapshot, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    }
    let mut corrupted = artifact.clone();
    assert!(edit_queue(&mut corrupted, &mut |queue| {
        queue["comparator"] = serde_json::json!({"kind":"unit"});
    }));
    let error = crate::api::codec::checkpoint_from_json(&corrupted, &checked).unwrap_err();
    assert!(
        error.message().contains("comparison must be a callable"),
        "{error}"
    );
    let mut missing = artifact;
    assert!(edit_queue(&mut missing, &mut |queue| {
        queue.as_object_mut().unwrap().remove("comparator");
    }));
    assert!(
        crate::api::codec::checkpoint_from_json(&missing, &checked)
            .unwrap_err()
            .message()
            .contains("missing its comparator")
    );
    for (name, expected) in [
        ("wrong_parameters", "checked slot type"),
        ("wrong_result", "checked slot type"),
        ("loud", "comparison must be pure"),
    ] {
        let item = checked
            .hir
            .items
            .iter()
            .find_map(|(item, data)| match data {
                etas_hir::HirItem::Flow(flow)
                    if checked.hir.symbols.get(flow.symbol).unwrap().name == name =>
                {
                    Some(item)
                }
                _ => None,
            })
            .unwrap();
        let mut bad =
            crate::api::codec::checkpoint_artifact_json(&["main.es".into()], "main", checkpoint)
                .unwrap();
        let target = crate::api::codec::value_json(&InterpValue::Callable(
            crate::control::CallTarget::FlowItem(item),
        ))
        .unwrap();
        assert!(edit_queue(&mut bad, &mut |queue| {
            queue["comparator"] = std::mem::take(&mut *target.clone());
        }));
        let error = crate::api::codec::checkpoint_from_json(&bad, &checked).unwrap_err();
        assert!(error.message().contains(expected), "{name}: {error}");
    }
}

fn edit_queue(root: &mut serde_json::Value, edit: &mut impl FnMut(&mut serde_json::Value)) -> bool {
    let mut pending = vec![root];
    while let Some(value) = pending.pop() {
        if value.get("kind").and_then(serde_json::Value::as_str) == Some("priority_queue") {
            edit(value);
            return true;
        }
        match value {
            serde_json::Value::Object(fields) => pending.extend(fields.values_mut()),
            serde_json::Value::Array(values) => pending.extend(values.iter_mut()),
            _ => {}
        }
    }
    false
}

#[tokio::test(flavor = "current_thread")]
async fn priority_queue_comparison_yields_to_cancellation_on_current_thread() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.limits.Iterations;
flow compare(a: i32, b: i32) -> i32 ![] { while true limit Iterations(10000000) {} return 0; }
flow main() -> unit {
    let queue = PriorityQueue.new<string, i32>(compare).push("a", 1).push("b", 2);
    return;
}
"#,
    );
    let host = FakeHost::new(availability(&[]));
    let run = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions::default(),
    );
    let control = run.control();
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(run.execute(), async {
            tokio::task::yield_now().await;
            control
                .stop(etas_host::execution::CancellationReason::Requested)
                .unwrap();
        })
    })
    .await
    .unwrap();
    let result = result.unwrap();
    assert!(
        matches!(result.outcome, crate::api::RunOutcome::Cancelled(_)),
        "{:?}",
        result.diagnostics
    );
    assert!(result.diagnostics.is_empty());
}

use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn registry_declared_list_queries_and_extend_execute_checked_source() {
    let mut failures = Vec::new();
    for (name, expression) in [
        ("head", "xs.head() == Some(1)"),
        ("tail", "xs.tail() == Some([2; 3]) && xs == [1; 2; 3]"),
        ("extend", "xs.extend([4; 5]) == [4; 5; 1; 2; 3]"),
        (
            "empty",
            "empty.head() == None && empty.tail() == None && xs.extend(empty) == xs && empty.extend(xs) == xs",
        ),
        (
            "singleton",
            "one.tail() == Some(empty) && one.head() == Some(2)",
        ),
    ] {
        let checked = checked_project(&format!(
            "module app.main; flow main() -> bool {{ let xs = [1; 2; 3]; let (one, _) = [1; 2].pop(); let (empty, _) = one.pop(); return {expression}; }}"
        ));
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![],
                &FakeHost::new(availability(&[])),
                RunOptions::default(),
            )
            .await
            .unwrap();
        if !result.diagnostics.is_empty() || result.value() != Some(&InterpValue::Bool(true)) {
            failures.push(format!(
                "{name}: {:?}; value={:?}",
                result.diagnostics,
                result.value()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test(flavor = "current_thread")]
async fn list_extend_suspended_prefix_preserves_aliases_and_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
flow prefix() -> List<string> {
    println("prefix");
    checkpoint("prefix");
    return ["a"; "b"];
}
flow main() -> bool {
    let suffix = ["c"; "d"];
    let alias = suffix;
    let joined = suffix.extend(prefix());
    checkpoint("joined");
    return joined == ["a"; "b"; "c"; "d"]
        && joined.head() == Some("a")
        && joined.tail() == Some(["b"; "c"; "d"])
        && alias == ["c"; "d"] && suffix == alias;
}
"#,
    );
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
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
    assert_eq!(host.stdout_text(), "prefix\n");
    assert_eq!(result.checkpoints.len(), 2);
    for saved in &result.checkpoints {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap();
        let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
        for checkpoint in [saved, &decoded] {
            let host = FakeHost::new(services);
            let resumed = Interpreter
                .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
                .await
                .unwrap();
            assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
            assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
            assert_eq!(host.stdout_text(), "");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn vector_and_ring_updates_preserve_nested_aliases_and_suspended_arguments() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
flow payload(label: string) -> Array<string> {
    println(label);
    checkpoint(label);
    return [label];
}
flow main() -> bool {
    var array = [["base"]];
    let original_array = array;
    let pushed_array = array.push(payload("array"));
    let stack = Stack.new<Array<string>>().push(["base"]);
    let pushed_stack = stack.push(payload("stack"));
    let queue = Queue.new<Array<string>>().push(["base"]);
    let pushed_queue = queue.push(payload("queue"));
    let deque = Deque.new<Array<string>>().push_back(["base"]);
    let pushed_deque = deque.push_front(payload("deque"));
    array[0][0] = "changed";
    checkpoint("updated");
    let (array_tail, array_item) = pushed_array.pop();
    let (stack_tail, stack_item) = pushed_stack.pop();
    let (_, stack_base) = stack_tail.pop();
    let (_, old_stack_base) = stack.pop();
    let (queue_tail, queue_base) = pushed_queue.pop();
    let (_, queue_item) = queue_tail.pop();
    let (_, old_queue_base) = queue.pop();
    let (deque_tail, deque_item) = pushed_deque.pop_front();
    let (_, deque_base) = deque_tail.pop_back();
    let (_, old_deque_base) = deque.pop_back();
    return array == [["changed"]] && original_array == [["base"]]
        && array_tail == original_array && array_item == Some(["array"])
        && stack_item == Some(["stack"]) && stack_base == Some(["base"]) && old_stack_base == stack_base
        && queue_item == Some(["queue"]) && queue_base == Some(["base"]) && old_queue_base == queue_base
        && deque_item == Some(["deque"]) && deque_base == Some(["base"]) && old_deque_base == deque_base;
}
"#,
    );
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
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
    assert_eq!(host.stdout_text(), "array\nstack\nqueue\ndeque\n");
    assert_eq!(result.checkpoints.len(), 5);
    for (checkpoint, expected) in result.checkpoints.iter().zip([
        "stack\nqueue\ndeque\n",
        "queue\ndeque\n",
        "deque\n",
        "",
        "",
    ]) {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
        let restored = checkpoint_from_json(&artifact, &checked).unwrap();
        for saved in [checkpoint, &restored] {
            let host = FakeHost::new(services);
            let invocation =
                Interpreter.create_resume(&checked, saved, &host, RunOptions::default());
            invocation
                .control()
                .stop(etas_host::execution::CancellationReason::Requested)
                .unwrap();
            let cancelled = invocation.execute().await.unwrap();
            assert!(matches!(
                cancelled.outcome,
                crate::api::RunOutcome::Cancelled(_)
            ));
            assert_eq!(host.stdout_text(), "");
            assert_eq!(
                checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap(),
                artifact
            );
            let resumed = Interpreter
                .resume_checkpoint(&checked, saved, &host, RunOptions::default())
                .await
                .unwrap();
            assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
            assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
            assert_eq!(host.stdout_text(), expected);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn array_concat_and_extend_preserve_aliases_and_suspended_operand_order() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
flow right(label: string) -> Array<string> {
    println(label);
    checkpoint(label);
    return ["right"];
}
flow main() -> bool {
    var base = ["left"];
    let original = base;
    let combined = base + right("plus");
    let extended = base.extend(right("extend"));
    base = base.push("changed");
    checkpoint("joined");
    return combined == ["left", "right"] && extended == combined
        && original == ["left"] && base == ["left", "changed"];
}
"#,
    );
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
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
    assert_eq!(host.stdout_text(), "plus\nextend\n");
    assert_eq!(result.checkpoints.len(), 3);
    for (index, checkpoint) in result.checkpoints.iter().enumerate() {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
        let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
        for saved in [checkpoint, &decoded] {
            let host = FakeHost::new(services);
            let resumed = Interpreter
                .resume_checkpoint(&checked, saved, &host, RunOptions::default())
                .await
                .unwrap();
            assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
            assert_eq!(resumed.value(), result.value());
            assert_eq!(host.stdout_text(), if index == 0 { "extend\n" } else { "" });
        }
    }
}
#[tokio::test(flavor = "current_thread")]
async fn persistent_list_updates_and_iteration_survive_handler_checkpoint_and_cancel() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
effect Select { action index() -> usize; }
flow main() -> bool {
    var nested = [["a", "b"]; ["c", "d"]];
    let alias = nested;
    var values = [1; 2; 3];
    let original = values;
    var total = 0;
    for value in values limit Iterations(8) {
        if value == 1 {
            values = values.push(9);
            total = total + value;
            continue;
        }
        if value == 2 { checkpoint("loop"); }
        total = total + value;
    }
    handle {
        nested[perform Select.index()][1] = "new";
    } with {
        Select.index() => { checkpoint("index"); resume 0; }
    };
    let combined = 0 :: original;
    let (tail, head) = combined.pop();
    let joined = original + [4; 5];
    return total == 6 && values == [9; 1; 2; 3]
        && head == Some(0) && tail == original && joined == [1; 2; 3; 4; 5]
        && nested == [["a", "new"]; ["c", "d"]]
        && alias == [["a", "b"]; ["c", "d"]];
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
    assert_eq!(result.checkpoints.len(), 2);
    for checkpoint in &result.checkpoints {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
        let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
        let invocation =
            Interpreter.create_resume(&checked, &checkpoint, &host, RunOptions::default());
        invocation
            .control()
            .stop(etas_host::execution::CancellationReason::Requested)
            .unwrap();
        let cancelled = invocation.execute().await.unwrap();
        assert!(matches!(
            cancelled.outcome,
            crate::api::RunOutcome::Cancelled(_)
        ));
        assert_eq!(
            checkpoint_artifact_json(&["main.es".into()], "main", &checkpoint).unwrap(),
            artifact
        );
        let resumed = Interpreter
            .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn queue_and_deque_order_and_aliases_survive_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow main() -> bool {
    let queue = Queue.new<i32>().push(1).push(2).push(3);
    let (tail, first) = queue.pop();
    let rotated = tail.push(4);
    let deque = Deque.new<i32>().push_back(2).push_front(1).push_back(3);
    let (tail_deque, back) = deque.pop_back();
    let (middle, front) = tail_deque.pop_front();
    checkpoint("containers");
    let (q2, second) = rotated.pop();
    let (q3, third) = q2.pop();
    let (q4, fourth) = q3.pop();
    let (_, missing) = q4.pop();
    let (_, original_first) = queue.pop();
    let (_, middle_value) = middle.pop_front();
    return first == Some(1) && back == Some(3) && front == Some(1)
        && second == Some(2) && third == Some(3) && fourth == Some(4)
        && missing == None && original_first == Some(1) && middle_value == Some(2);
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
    assert_eq!(result.checkpoints.len(), 1);
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn collection_owned_arguments_resume_once_in_source_order() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
flow step(label: string) -> string {
    println(label);
    checkpoint(label);
    return label;
}
flow main() -> bool {
    let base = ["base"];
    let pushed = base.push(step("push"));
    let extended = pushed.extend([step("extend")]);
    let initial = OrderedMap.new<string, string>();
    let map = initial.insert(step("key"), step("value"));
    return base == ["base"] && pushed == ["base", "push"]
        && extended == ["base", "push", "extend"]
        && initial.get("key") == None && map.get("key") == Some("value");
}
"#,
    );
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
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
    assert_eq!(host.stdout_text(), "push\nextend\nkey\nvalue\n");
    assert_eq!(result.checkpoints.len(), 4);
    for (checkpoint, expected) in
        result
            .checkpoints
            .iter()
            .zip(["extend\nkey\nvalue\n", "key\nvalue\n", "value\n", ""])
    {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
        let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
        let host = FakeHost::new(services);
        let result = Interpreter
            .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(host.stdout_text(), expected);
    }
}

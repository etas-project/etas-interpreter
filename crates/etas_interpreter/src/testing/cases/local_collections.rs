use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

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

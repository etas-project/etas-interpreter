use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn conditional_resume_executes_only_the_reachable_branch() {
    let checked = checked_project(
        r#"
module app.main;
effect Solver { action ask() -> i32; }
flow main(cached: bool) -> i32 {
    return perform Solver.ask() with {
        Solver.ask() => { if cached { resume 1; } resume 2; }
    };
}
"#,
    );
    for (cached, expected) in [(true, 1), (false, 2)] {
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![value::InterpValue::Bool(cached)],
                &FakeHost::new(HostServiceAvailability::default()),
                RunOptions::default(),
            )
            .await
            .expect("execution lifecycle");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(
            result.value().cloned(),
            Some(value::InterpValue::i32(expected))
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn string_ordering_is_preserved_by_checkpoint_replay() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow main() -> bool {
    let prefix = "中";
    let left = prefix + "😀";
    let right = prefix + "😁";
    checkpoint("before-ordering");
    return left < right && right >= left && left <= "中😀";
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
        .expect("execution lifecycle");
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.value().cloned(), Some(value::InterpValue::Bool(true)));
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &first.checkpoints[0]).unwrap();
    let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .expect("resume lifecycle");
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), first.value());
}

#[tokio::test(flavor = "current_thread")]
async fn tail_control_values_and_nested_record_conditions_execute() {
    let checked = checked_project(
        r#"
module app.main;
type Session = { value: i32 };
effect Probe { action ask() -> i32; }
flow exercise(session: Session) -> bool { return session.value == 7; }
flow choose(first: bool, second: bool) -> i32 {
    if first { 1 } else if second { 2 } else {
        match first { true => 3, false => 4 }
    }
}
flow main() -> i32 {
    if !exercise(Session { value = 7 }) { return 99; }
    let handled = handle {
        if true { choose(false, false) } else { 99 }
    } with { Probe.ask() => { resume 0; } };
    return handled + choose(false, true);
}
"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value().cloned(), Some(value::InterpValue::i32(6)));
}

#[tokio::test(flavor = "current_thread")]
async fn string_ordering_accepts_flat_segmented_and_unicode_values() {
    let checked = checked_project(
        r#"
module app.main;
flow main() -> bool {
    let prefix = "中";
    let key = prefix + "😀";
    let same = "中😀";
    return "a" < key && key > "a" && key <= same && key >= same
        && !(key < same) && !(key > same) && "" < key
        && "é" > "e" && "中" < key;
}
"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::Bool(true))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn inner_handler_remains_active_after_outer_handler_resumes() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.println;
effect Stages { action main() -> i32; }
effect Progress { action write(text: string) -> unit; }
flow body() -> i32 ![Stages, Progress] {
    perform Progress.write("before");
    let value = perform Stages.main();
    perform Progress.write("after");
    return value;
}
flow facade() -> i32 ![Stages, Console, Error<IOError>] {
    return body() with {
        Progress.write(text) => { println(text); resume; }
    };
}
flow main() -> i32 ![Console, Error<IOError>] {
    return facade() with { Stages.main() => { resume 42; } };
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
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value().cloned(), Some(value::InterpValue::i32(42)));
    assert_eq!(host.stdout_text(), "before\nafter\n");
    assert_eq!(host.console_call_count(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn forwarded_handler_is_suspended_during_outer_arm_and_restored_after_checkpoint() {
    for (completion, expected, output, resumed_output) in [
        (
            "resume 42;",
            42,
            "inner:before\nroot:outer\nmarker:after\ninner:after\n",
            "marker:after\ninner:after\n",
        ),
        ("finish 17;", 17, "inner:before\nroot:outer\n", ""),
    ] {
        let checked = checked_project(&format!(
            r#"
module app.main;
import std.effects.Console;
import std.io.println;
import std.runtime.checkpoint;
effect Stages {{ action main() -> i32; }}
effect Progress {{ action write(text: string) -> unit; }}
effect Marker {{ action write(text: string) -> unit; }}
effect Temporary {{ action ask() -> unit; }}
flow body() -> i32 ![Stages, Progress, Marker] {{
    perform Progress.write("before");
    let value = perform Stages.main();
    perform Marker.write("after");
    perform Progress.write("after");
    return value;
}}
flow progress_facade() -> i32 ![Stages, Marker, Console, Error<IOError>] {{
    return body() with {{ Progress.write(text) => {{ println("inner:" + text); resume; }} }};
}}
flow facade() -> i32 ![Stages, Console, Error<IOError>] {{
    return progress_facade() with {{ Marker.write(text) => {{ println("marker:" + text); resume; }} }};
}}
flow main() -> i32 ![Console, Error<IOError>] {{
    return handle {{
        return facade() with {{ Stages.main() => {{
            perform Progress.write("outer");
            checkpoint("suspended-inner");
            perform Temporary.ask() with {{ Temporary.ask() => {{ checkpoint("temporary"); resume; }} }};
            {completion}
        }} }};
    }} with {{ Progress.write(text) => {{ println("root:" + text); resume; }} }};
}}
"#
        ));
        let host = FakeHost::new(availability(&[
            HostRequirementKind::Console,
            HostRequirementKind::Checkpoint,
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
            .expect("execution lifecycle");
        assert!(
            first.diagnostics.is_empty(),
            "{completion}: {:?}",
            first.diagnostics
        );
        assert_eq!(
            first.value().cloned(),
            Some(value::InterpValue::i32(expected))
        );
        assert_eq!(host.stdout_text(), output);
        let artifact =
            checkpoint_artifact_json(&["main.es".into()], "main", &first.checkpoints[0]).unwrap();
        let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
        fn reverse_reentry_order(value: &mut serde_json::Value, next: &mut u32) -> usize {
            let mut count = 0;
            match value {
                serde_json::Value::Object(object) => {
                    if object.get("kind").and_then(serde_json::Value::as_str)
                        == Some("reenter_handler")
                    {
                        object.get_mut("boundary").unwrap()["scope_id"] = (*next).into();
                        *next -= 1;
                        count += 1;
                    }
                    count += object
                        .values_mut()
                        .map(|value| reverse_reentry_order(value, next))
                        .sum::<usize>();
                }
                serde_json::Value::Array(array) => {
                    count += array
                        .iter_mut()
                        .map(|value| reverse_reentry_order(value, next))
                        .sum::<usize>();
                }
                _ => {}
            }
            count
        }
        let mut reordered = artifact.clone();
        assert!(reverse_reentry_order(&mut reordered, &mut (u32::MAX - 1)) >= 2);
        assert!(
            checkpoint_from_json(&reordered, &checked)
                .unwrap_err()
                .message()
                .contains("suspended handler scope order")
        );
        fn corrupt_reentry(value: &mut serde_json::Value) -> bool {
            match value {
                serde_json::Value::Object(object)
                    if object.get("kind").and_then(serde_json::Value::as_str)
                        == Some("reenter_handler") =>
                {
                    object.insert(
                        "boundary".into(),
                        serde_json::json!({"kind": "block_value"}),
                    );
                    true
                }
                serde_json::Value::Object(object) => object.values_mut().any(corrupt_reentry),
                serde_json::Value::Array(array) => array.iter_mut().any(corrupt_reentry),
                _ => false,
            }
        }
        let mut malformed = artifact.clone();
        assert!(
            corrupt_reentry(&mut malformed),
            "must corrupt a real saved handler reentry"
        );
        assert!(
            checkpoint_from_json(&malformed, &checked)
                .unwrap_err()
                .message()
                .contains("handler reentry does not contain a handler boundary")
        );
        let host = FakeHost::new(availability(&[
            HostRequirementKind::Console,
            HostRequirementKind::Checkpoint,
        ]));
        let resumed = Interpreter
            .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
            .await
            .expect("resume lifecycle");
        assert!(
            resumed.diagnostics.is_empty(),
            "{completion}: {:?}",
            resumed.diagnostics
        );
        assert_eq!(
            resumed.value().cloned(),
            Some(value::InterpValue::i32(expected))
        );
        assert_eq!(host.stdout_text(), resumed_output);
        for saved in &resumed.checkpoints {
            let artifact = checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap();
            checkpoint_from_json(&artifact, &checked)
                .expect("restored dormant scope IDs must not be reused by new handlers");
        }
    }
}

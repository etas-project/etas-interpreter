use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

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
async fn ordered_comparison_checks_dynamic_callback_purity_before_host_dispatch() {
    for (name, body, succeeds) in [
        ("quiet", "return;", true),
        ("loud", "println(\"must not run\")?; return;", false),
    ] {
        for constructor in [
            "OrderedSet.new<() -> unit ![]>(compare).insert(key).insert(key)",
            "OrderedMap.new<() -> unit ![], i32>(compare).insert(key, 1).insert(key, 2)",
            "PriorityQueue.new<i32, () -> unit ![]>(compare).push(1, key).push(2, key)",
        ] {
            let checked = checked_project(&format!(
                r#"
module app.main;
import std.io.println;
flow {name}() -> unit ![] {{ {body} }}
flow compare(a: () -> unit ![], b: () -> unit ![]) -> i32 ![] {{
    a(); b(); return 0;
}}
flow main() -> bool {{
    println("before")?;
    let key = {name};
    let collection = {constructor};
    return true;
}}
"#
            ));
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
            if succeeds {
                assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
                assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
            } else {
                assert!(result.value().is_none());
                assert!(
                    result
                        .diagnostics
                        .iter()
                        .any(|d| d.message.contains("pure comparison cannot request effects")),
                    "{:?}",
                    result.diagnostics
                );
            }
            assert_eq!(host.stdout_text(), "before\n");
            assert_eq!(host.console_call_count(), 1);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_default_comparison_rejects_even_the_first_nominal_key() {
    for constructor in [
        "OrderedMap.new<Key, i32>().insert(key, 1)",
        "OrderedSet.new<Key>().insert(key)",
    ] {
        let checked = checked_project(&format!(
            r#"
module app.main;
type Key = i32;
flow main() -> unit {{
    let key = Key(1);
    let collection = {constructor};
    return;
}}
"#
        ));
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
        assert!(result.value().is_none());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("comparison flow")),
            "{:?}",
            result.diagnostics
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_collections_preserve_constructor_and_argument_effect_order() {
    for (construction, expected) in [
        (
            "OrderedMap.new<i32, string>(comparison()).insert(1, \"old\").insert(key(), value())",
            "comparison\nkey\nvalue\n",
        ),
        (
            "OrderedSet.new<i32>(comparison()).insert(1).insert(key())",
            "comparison\nkey\n",
        ),
    ] {
        let checked = checked_project(&format!(
            r#"
module app.main;
import std.io.println;
flow comparison() -> (i32, i32) -> i32 ![] {{
    println("comparison")?;
    let direction: i32 = -1;
    return (a: i32, b: i32) => {{
        if a < b {{ return 0 - direction; }}
        if a > b {{ return direction; }}
        return 0;
    }};
}}
flow key() -> i32 {{ println("key")?; return 2; }}
flow value() -> string {{ println("value")?; return "new"; }}
flow main() -> bool {{ let collection = {construction}; return true; }}
"#
        ));
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
        assert_eq!(host.stdout_text(), expected);
        assert_eq!(host.console_call_count(), expected.lines().count());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_collections_reject_default_handled_actions_in_comparisons() {
    for constructor in ["OrderedMap.new<i32, string>", "OrderedSet.new<i32>"] {
        let checked = checked_project(&format!(
            r#"
module app.main;
import std.io.println;
flow compare(a: i32, b: i32) -> i32 ![] {{ println("must not run")?; return 0; }}
flow main() -> unit {{ let collection = {constructor}(compare); return; }}
"#
        ));
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
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_comparison_consumes_execution_fuel() {
    let checked = checked_project(
        r#"
module app.main;
flow compare(a: i32, b: i32) -> i32 ![] {
    while true limit Iterations(10000000) {}
    return 0;
}
flow main() -> unit {
    let collection = OrderedSet.new<i32>(compare).insert(1).insert(2);
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
async fn ordered_comparison_yields_to_cancellation_on_current_thread() {
    let checked = checked_project(
        r#"
module app.main;
flow compare(a: i32, b: i32) -> i32 ![] {
    while true limit Iterations(10000000) {}
    return 0;
}
flow main() -> unit {
    let collection = OrderedSet.new<i32>(compare).insert(1).insert(2);
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

#[tokio::test(flavor = "current_thread")]
async fn ordered_builtin_checkpoints_restore_order_and_old_aliases() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow main() -> bool {
    let old = OrderedMap.new<i32, string>().insert(2, "two").insert(1, "one");
    let map = old.insert(2, "changed").insert(3, "three");
    let set = OrderedSet.new<i32>().insert(3).insert(1).insert(2).insert(1);
    checkpoint("ordered");
    var map_order = 0;
    var set_order = 0;
    for (key, value) in map limit Iterations(8) { map_order = map_order * 10 + key; }
    for key in set limit Iterations(8) { set_order = set_order * 10 + key; }
    return map_order == 123 && set_order == 123 && old.get(2) == Some("two") && map.get(2) == Some("changed");
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
    let wire =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    let decoded = checkpoint_from_json(&wire, &checked).unwrap();
    for checkpoint in [&result.checkpoints[0], &decoded] {
        let resumed = Interpreter
            .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), result.value());
    }
    for kind in ["ordered_map", "ordered_set"] {
        let mut corrupted = wire.clone();
        let mut pending = vec![&mut corrupted];
        let mut edited = false;
        while let Some(value) = pending.pop() {
            if value.get("kind").and_then(serde_json::Value::as_str) == Some(kind) {
                let name = if kind == "ordered_map" {
                    "entries"
                } else {
                    "values"
                };
                value[name].as_array_mut().unwrap().reverse();
                edited = true;
                break;
            }
            match value {
                serde_json::Value::Object(fields) => pending.extend(fields.values_mut()),
                serde_json::Value::Array(values) => pending.extend(values.iter_mut()),
                _ => (),
            }
        }
        assert!(edited);
        assert!(
            checkpoint_from_json(&corrupted, &checked)
                .unwrap_err()
                .message()
                .contains("out-of-order")
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_collections_traverse_keys_not_insertion_order() {
    for keys in [
        [1, 2, 3],
        [1, 3, 2],
        [2, 1, 3],
        [2, 3, 1],
        [3, 1, 2],
        [3, 2, 1],
    ] {
        let map = keys
            .iter()
            .map(|key| format!(".insert({key}, {key} * 10)"))
            .collect::<String>();
        let set = keys
            .iter()
            .map(|key| format!(".insert({key})"))
            .collect::<String>();
        assert_true(&format!(
            r#"
module app.main;
flow main() -> bool {{
    let map = OrderedMap.new<i32, i32>(){map};
    let set = OrderedSet.new<i32>(){set};
    var map_order = 0;
    var set_order = 0;
    for (key, value) in map limit Iterations(8) {{ map_order = map_order * 10 + key; }}
    for key in set limit Iterations(8) {{ set_order = set_order * 10 + key; }}
    return map_order == 123 && set_order == 123;
}}
"#
        ))
        .await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_collections_replace_and_deduplicate_without_mutating_aliases() {
    assert_true(
        r#"
module app.main;
flow main() -> bool {
    let old = OrderedMap.new<i32, string>().insert(2, "old").insert(1, "first");
    let next = old.insert(2, "new").insert(3, "last");
    let original = OrderedSet.new<i32>().insert(2).insert(1);
    let added = original.insert(2).insert(3);
    var order = 0;
    var old_count = 0;
    var next_count = 0;
    var set_count = 0;
    for (key, value) in next limit Iterations(8) { order = order * 10 + key; }
    for (key, value) in old limit Iterations(8) { old_count = old_count + 1; }
    for (key, value) in next limit Iterations(8) { next_count = next_count + 1; }
    for key in added limit Iterations(8) { set_count = set_count + 1; }
    return order == 123 && old.get(2) == Some("old") && next.get(2) == Some("new")
        && old_count == 2 && next_count == 3 && set_count == 3
        && !original.contains(3) && added.contains(3);
}
"#,
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_collections_use_custom_comparison_for_nested_nominal_keys() {
    assert_true(r#"
module app.main;
type Key = { parts: (i32, string) };
flow reverse(a: Key, b: Key) -> i32 ![] {
    let (left, _) = a.parts;
    let (right, _) = b.parts;
    if left > right { return -1; }
    if left < right { return 1; }
    return 0;
}
flow main() -> bool {
    let first = Key { parts = (1, "original") };
    let same = Key { parts = (1, "equivalent") };
    let second = Key { parts = (2, "second") };
    let old = OrderedMap.new<Key, string>(reverse).insert(first, "old").insert(second, "second");
    let next = old.insert(same, "replacement");
    let set = OrderedSet.new<Key>(reverse).insert(first).insert(second).insert(same);
    var map_order = 0;
    var set_order = 0;
    for (key, value) in next limit Iterations(8) {
        let (rank, _) = key.parts;
        map_order = map_order * 10 + rank;
    }
    for key in set limit Iterations(8) {
        let (rank, _) = key.parts;
        set_order = set_order * 10 + rank;
    }
    return map_order == 21 && set_order == 21 && next.get(first) == Some("replacement")
        && next.get(same) == Some("replacement") && old.get(same) == Some("old") && set.contains(same);
}
"#).await;
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_collections_support_captured_comparisons_and_empty_queries() {
    assert_true(
        r#"
module app.main;
flow main() -> bool {
    let direction: i32 = -1;
    let compare: (i32, i32) -> i32 ![] = (a: i32, b: i32) => {
        if a < b { return 0 - direction; }
        if a > b { return direction; }
        return 0;
    };
    let empty = OrderedMap.new<i32, string>(compare);
    let set = OrderedSet.new<i32>(compare);
    let filled = empty.insert(1, "one").insert(3, "three").insert(2, "two");
    var order = 0;
    for (key, value) in filled limit Iterations(8) { order = order * 10 + key; }
    return empty.get(1) == None && !empty.contains_key(1) && !set.contains(1) && order == 321;
}
"#,
    )
    .await;
}

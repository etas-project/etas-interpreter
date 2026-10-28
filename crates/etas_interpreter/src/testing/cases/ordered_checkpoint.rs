use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

const SOURCE: &str = r#"
module app.main;
import std.runtime.checkpoint;
import std.io.println;
flow wrong_inputs(a: string, b: string) -> i32 ![] { return 0; }
flow wrong_output(a: i32, b: i32) -> string ![] { return "wrong"; }
flow equivalent(a: i32, b: i32) -> i32 ![] { return 0; }
flow loud(a: i32, b: i32) -> i32 ![] { println("must not run")?; return 0; }
flow forever(a: i32, b: i32) -> i32 ![] {
    while true limit Iterations(10000000) {}
    return 0;
}
flow comparison(direction: i32) -> (i32, i32) -> i32 ![] {
    return (a: i32, b: i32) => {
        if a < b { return 0 - direction; }
        if a > b { return direction; }
        return 0;
    };
}
flow main() -> bool {
    let compare = comparison(-1);
    let old = OrderedMap.new<i32, string>(compare).insert(1, "one").insert(3, "three").insert(2, "old");
    let map = old.insert(2, "changed");
    let set = OrderedSet.new<i32>(compare).insert(1).insert(3).insert(2).insert(2);
    checkpoint("custom-order");
    let next = map.insert(4, "four");
    var map_order = 0;
    var set_order = 0;
    for (key, value) in next limit Iterations(8) { map_order = map_order * 10 + key; }
    for key in set limit Iterations(8) { set_order = set_order * 10 + key; }
    return map_order == 4321 && set_order == 321 && old.get(2) == Some("old")
        && map.get(2) == Some("changed") && !map.contains_key(4) && set.contains(2);
}
"#;

async fn fixture() -> (
    etas_frontend::CheckedProject,
    crate::api::RunResult,
    FakeHost,
) {
    let checked = checked_project(SOURCE);
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
    (checked, result, host)
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_checkpoint_preserves_captured_comparison_order_and_aliases() {
    let (checked, result, host) = fixture().await;
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
    assert_eq!(host.console_call_count(), 0);
}

fn edit(
    root: &mut serde_json::Value,
    kind: &str,
    change: &mut impl FnMut(&mut serde_json::Value),
) -> bool {
    let mut work = vec![root];
    while let Some(value) = work.pop() {
        if value.get("kind").and_then(serde_json::Value::as_str) == Some(kind) {
            change(value);
            return true;
        }
        match value {
            serde_json::Value::Object(fields) => work.extend(fields.values_mut()),
            serde_json::Value::Array(values) => work.extend(values.iter_mut()),
            _ => (),
        }
    }
    false
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_checkpoint_rejects_reordered_keys_before_resuming_source() {
    let (checked, result, host) = fixture().await;
    for kind in ["ordered_map", "ordered_set"] {
        let mut wire =
            checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
        assert!(edit(&mut wire, kind, &mut |value| {
            let field = if kind == "ordered_map" {
                "entries"
            } else {
                "values"
            };
            value[field].as_array_mut().unwrap().reverse();
        }));
        let checkpoint = checkpoint_from_json(&wire, &checked).unwrap();
        let resumed = Interpreter
            .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.value().is_none());
        assert!(
            resumed
                .diagnostics
                .iter()
                .any(|d| d.message.contains("out-of-order")),
            "{:?}",
            resumed.diagnostics
        );
    }
    assert_eq!(host.console_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_checkpoint_rejects_invalid_or_missing_comparison_abi_and_effects() {
    let (checked, result, host) = fixture().await;
    for kind in ["ordered_map", "ordered_set"] {
        for (name, reason) in [
            ("wrong_inputs", "checked slot type"),
            ("wrong_output", "checked slot type"),
            ("loud", "comparison must be pure"),
        ] {
            let item = flow(&checked, name);
            let comparator = crate::api::codec::value_json(&InterpValue::Callable(
                crate::control::CallTarget::FlowItem(item),
            ))
            .unwrap();
            let mut wire =
                checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0])
                    .unwrap();
            assert!(edit(&mut wire, kind, &mut |value| {
                value["comparator"] = std::mem::take(&mut *comparator.clone());
            }));
            let error = checkpoint_from_json(&wire, &checked).unwrap_err();
            assert!(error.message().contains(reason), "{name}: {error}");
        }
        for missing in [false, true] {
            let mut wire =
                checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0])
                    .unwrap();
            assert!(edit(&mut wire, kind, &mut |value| {
                if missing {
                    value.as_object_mut().unwrap().remove("comparator");
                } else {
                    value["comparator"] = serde_json::json!({"kind":"unit"});
                }
            }));
            let error = checkpoint_from_json(&wire, &checked).unwrap_err();
            assert!(
                error.message().contains(if missing {
                    "missing its comparator"
                } else {
                    "comparison must be a callable"
                }),
                "{error}"
            );
        }
    }
    assert_eq!(host.console_call_count(), 0);
}

fn flow(checked: &etas_frontend::CheckedProject, name: &str) -> etas_hir::HirItemId {
    checked
        .hir
        .items
        .iter()
        .find_map(|(item, declaration)| match declaration {
            etas_hir::HirItem::Flow(flow)
                if checked.symbols.get(flow.symbol).unwrap().name == name =>
            {
                Some(item)
            }
            _ => None,
        })
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_checkpoint_validation_consumes_run_fuel() {
    let (checked, result, host) = fixture().await;
    let comparator = crate::api::codec::value_json(&InterpValue::Callable(
        crate::control::CallTarget::FlowItem(flow(&checked, "forever")),
    ))
    .unwrap();
    let mut wire =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    assert!(edit(&mut wire, "ordered_map", &mut |value| {
        value["comparator"] = std::mem::take(&mut *comparator.clone());
    }));
    let checkpoint = checkpoint_from_json(&wire, &checked).unwrap();
    let mut options = RunOptions::default();
    options.execution_limits.max_steps =
        std::num::NonZeroU64::new(checkpoint.execution_progress.consumed_steps + 128);
    let resumed = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        Interpreter.resume_checkpoint(&checked, &checkpoint, &host, options),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(resumed.value().is_none());
    assert!(
        resumed
            .diagnostics
            .iter()
            .any(|d| d.message.contains("step")),
        "{:?}",
        resumed.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_checkpoint_validation_yields_to_cancellation() {
    let (checked, result, host) = fixture().await;
    let comparator = crate::api::codec::value_json(&InterpValue::Callable(
        crate::control::CallTarget::FlowItem(flow(&checked, "forever")),
    ))
    .unwrap();
    let mut wire =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    assert!(edit(&mut wire, "ordered_map", &mut |value| {
        value["comparator"] = std::mem::take(&mut *comparator.clone());
    }));
    let checkpoint = checkpoint_from_json(&wire, &checked).unwrap();
    let run = Interpreter.create_resume(&checked, &checkpoint, &host, RunOptions::default());
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
    assert_eq!(host.console_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_checkpoint_rejects_keys_that_the_comparison_equates() {
    let (checked, result, host) = fixture().await;
    let comparator = crate::api::codec::value_json(&InterpValue::Callable(
        crate::control::CallTarget::FlowItem(flow(&checked, "equivalent")),
    ))
    .unwrap();
    for kind in ["ordered_map", "ordered_set"] {
        let mut wire =
            checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
        assert!(edit(&mut wire, kind, &mut |value| {
            value["comparator"] = std::mem::take(&mut *comparator.clone());
        }));
        let checkpoint = checkpoint_from_json(&wire, &checked).unwrap();
        let resumed = Interpreter
            .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.value().is_none());
        assert!(
            resumed
                .diagnostics
                .iter()
                .any(|d| d.message.contains("duplicate")),
            "{:?}",
            resumed.diagnostics
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_checkpoint_restores_custom_comparison_in_entry_arguments() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow compare(a: i32, b: i32) -> i32 ![] {
    if a < b { return 1; }
    if a > b { return -1; }
    return 0;
}
flow main(map: OrderedMap<i32, string>) -> bool {
    checkpoint("argument");
    let next = map.insert(3, "three");
    var order = 0;
    for (key, value) in next limit Iterations(8) { order = order * 10 + key; }
    return order == 321 && map.get(2) == Some("two") && !map.contains_key(3);
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let tree = crate::value::ordered::OrderedTree::from_pending_entries(vec![
        (
            InterpValue::Number(crate::value::NumericValue::I32(2)),
            InterpValue::String("two".into()),
        ),
        (
            InterpValue::Number(crate::value::NumericValue::I32(1)),
            InterpValue::String("one".into()),
        ),
    ])
    .unwrap();
    let argument = InterpValue::OrderedMap(crate::value::ordered::OrderedMapValue::from_tree(
        tree,
        Some(crate::control::CallTarget::FlowItem(flow(
            &checked, "compare",
        ))),
    ));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![argument],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
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
}

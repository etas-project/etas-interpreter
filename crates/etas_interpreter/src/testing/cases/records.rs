use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn nominal_record_equality_by_field_name_checks_nested_generic_values() {
    let checked = checked_project(
        r#"
module app.main;
type State = { ready: bool, label: string }
type Report<T> = { status: T, state: State }
flow expected() -> Report<i32> {
    return Report<i32> { status = 200, state = State { ready = true, label = "ok" } };
}
flow actual() -> Report<i32> {
    return Report<i32> { state = State { label = "ok", ready = true }, status = 200 };
}
flow main() -> bool {
    let left = expected();
    let right = actual();
    let different = Report<i32> { state = right.state, status = 503 };
    return left.status == right.status && left.state.ready == right.state.ready
        && left.state.label == right.state.label && left.state == right.state
        && left == right && !(left != right) && left != different
        && [left] == [right] && Some(left) == Some(right) && #{left, right} == #{right};
}
"#,
    );
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
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn nominal_record_equality_by_field_name_preserves_effect_order_and_checkpoint_keys() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
import std.collections.contains_key;
type Row = { status: i32, state: i32 }
flow field(label: string, value: i32) -> i32 { println(label); return value; }
flow main() -> bool {
    let left = Row { status = field("status", 200), state = field("state", 1) };
    let right = Row { state = field("state", 1), status = field("status", 200) };
    let keys = #{left, right};
    let map: Map<Row, string> = { left => "kept" };
    checkpoint("records");
    var count = 0;
    for key in keys limit Iterations(4) { count = count + 1; }
    return left == right && count == 1 && map.contains_key(right) && contains_key(map, right)
        && map.get(right) == Some("kept");
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]));
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
    assert_eq!(host.stdout_text(), "status\nstate\nstate\nstatus\n");
    assert_eq!(result.checkpoints.len(), 1);
    let saved = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    for checkpoint in [saved, &decoded] {
        let host = FakeHost::new(availability(&[
            HostRequirementKind::Console,
            HostRequirementKind::Checkpoint,
        ]));
        let resumed = Interpreter
            .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(host.stdout_text(), "");
    }
    assert_eq!(
        checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap(),
        artifact
    );

    fn duplicate_reordered_record_member(value: &mut serde_json::Value) -> usize {
        match value {
            serde_json::Value::Object(object) => {
                if object.get("kind").and_then(serde_json::Value::as_str) == Some("set") {
                    let values = object.get_mut("values").unwrap().as_array_mut().unwrap();
                    let mut reordered = values[0].clone();
                    let fields = reordered["value"]["fields"].as_array_mut().unwrap();
                    fields.reverse();
                    assert_ne!(values[0], reordered);
                    values.push(reordered);
                    return 1;
                }
                object
                    .values_mut()
                    .map(duplicate_reordered_record_member)
                    .sum()
            }
            serde_json::Value::Array(values) => values
                .iter_mut()
                .map(duplicate_reordered_record_member)
                .sum(),
            _ => 0,
        }
    }
    let mut malformed = artifact;
    assert!(duplicate_reordered_record_member(&mut malformed) > 0);
    assert!(
        checkpoint_from_json(&malformed, &checked)
            .unwrap_err()
            .message()
            .contains("duplicate set element")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checked_record_layouts_preserve_source_order_aliases_and_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
type Row<T> = { a: T, b: T, c: T }
alias View<T> = Row<T>;
flow field(label: string, value: i32) -> i32 {
    println(label);
    checkpoint(label);
    return value;
}
flow identity(row: View<i32>) -> Row<i32> { return row; }
flow main() -> i32 {
    let row = Row<i32> { c = field("c", 3), a = field("a", 1), b = field("b", 2) };
    let second = identity(row).b;
    return match row {
        { c: 4 } => 0,
        { c, a, b } => a * 100 + second * 10 + c,
    };
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]));
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
    assert_eq!(result.value(), Some(&InterpValue::i32(123)));
    assert_eq!(host.stdout_text(), "c\na\nb\n");
    assert_eq!(result.checkpoints.len(), 3);
    for snapshot in &result.checkpoints {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", snapshot).unwrap();
        let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
        let resumed = Interpreter
            .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::i32(123)));
    }
}

#[test]
fn checked_record_plan_rejects_unknown_projection_and_pattern_fields() {
    let source = r#"
module app.main;
type Row = { a: i32, b: i32 }
flow row() -> Row { return Row { b = 2, a = 1 }; }
flow main() -> i32 {
    let field = row().a;
    return match row() { { b } => b + field };
}


"#;
    for corrupt_pattern in [false, true] {
        let mut checked = checked_project(source);
        crate::testing::edit_checked_hir(&mut checked, |hir| {
            if corrupt_pattern {
                let pat = hir
                    .pats
                    .iter()
                    .find_map(|(pat, data)| {
                        matches!(data, etas_hir::HirPat::Record { .. }).then_some(pat)
                    })
                    .unwrap();
                let etas_hir::HirPat::Record { fields, .. } = hir.pats.get_mut(pat).unwrap() else {
                    unreachable!();
                };
                fields[0].name = "unknown".into();
            } else {
                let expr = hir
                    .exprs
                    .iter()
                    .find_map(|(expr, data)| {
                        matches!(data, etas_hir::HirExpr::Field { .. }).then_some(expr)
                    })
                    .unwrap();
                let etas_hir::HirExpr::Field { field, .. } = hir.exprs.get_mut(expr).unwrap()
                else {
                    unreachable!();
                };
                *field = "unknown".into();
            }
        })
        .unwrap();
        let result = Interpreter.plan(&checked, PlanOptions);
        assert!(result.plan.is_none());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("unknown")),
            "{:?}",
            result.diagnostics
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn checked_nested_record_paths_use_materialized_projection_types() {
    let source = r#"
module app.main;
import std.agent.message.Message;
import std.runtime.checkpoint;
type Inner<T> = { value: T, padding: i32 }
type Outer<T> = { inner: Inner<T>, other: i32 }
flow nested<T>(row: Outer<T>) -> T { return row.inner.value; }
flow main() -> string {
    let row = Outer<string> { other = 0, inner = Inner<string> { padding = 1, value = "kept" } };
    let message = Message.new(row);
    checkpoint("nested");
    return nested(row) + message.body.inner.value;
}
"#;
    let checked = checked_project(source);
    let chain = checked
        .types
        .field_projections
        .values()
        .find(|fields| fields.len() == 3)
        .unwrap();
    assert_eq!(
        chain.iter().map(|f| f.field.as_str()).collect::<Vec<_>>(),
        ["body", "inner", "value"]
    );
    for pair in chain.windows(2) {
        assert_eq!(pair[0].output, pair[1].receiver);
    }
    assert_eq!(
        checked.type_store.get(chain[2].output),
        Some(&etas_types::Type::Primitive(
            etas_types::PrimitiveType::String
        ))
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
    assert_eq!(
        result.value(),
        Some(&InterpValue::String("keptkept".into()))
    );
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), result.value());

    let mut incomplete = checked.clone();
    std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut incomplete).types)
        .field_projections
        .clear();
    let rejected = Interpreter.plan(&incomplete, PlanOptions);
    assert!(rejected.plan.is_none());
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|d| d.message.contains("projection fact")),
        "{:?}",
        rejected.diagnostics
    );
}

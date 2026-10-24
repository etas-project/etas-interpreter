use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn list_cons_preserves_shared_tail_across_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow head() -> string {
    checkpoint("head");
    return "head";
}
flow main() -> List<List<string>> {
    let tail = ["a"; "b"];
    let combined = head() :: tail;
    let unique = "head" :: ["a"; "b"];
    return [combined; tail; unique];
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
    let list = |strings: &[&str]| {
        InterpValue::List(
            strings
                .iter()
                .map(|s| InterpValue::String((*s).into()))
                .collect::<Vec<_>>()
                .into(),
        )
    };
    let expected = InterpValue::List(
        vec![
            list(&["head", "a", "b"]),
            list(&["a", "b"]),
            list(&["head", "a", "b"]),
        ]
        .into(),
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&expected));
    assert_eq!(result.checkpoints.len(), 1);
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&expected));
}

#[tokio::test(flavor = "current_thread")]
async fn huge_range_slice_and_early_break_execute_without_materialization() {
    let checked = checked_project(
        r#"
module app.main;
flow main() -> u128 {
    let all: Range<u128> = [0, 340282366920938463463374607431768211455);
    let window = all[3, 6);
    var sum: u128 = 0;
    for value in window limit Iterations(10) { sum = sum + value; }
    for value in all limit Iterations(4294967295) { sum = sum + value; break; }
    return sum;
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
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::Number(crate::value::NumericValue::U128(12)))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn range_iteration_and_slicing_include_integer_maximum_without_overflow() {
    let checked = checked_project(
        r#"
module app.main;
flow main() -> u8 {
    let all: Range<u8> = Range.closed(254, 255);
    let window = all[1, 2);
    var last: u8 = 0;
    for value in window limit Iterations(8) { last = value; }
    return last;
}
"#,
    );
    for (expr, data) in checked.hir.exprs.iter() {
        if let etas_hir::HirExpr::Literal(etas_hir::HirLiteral::Int { text, .. }) = data
            && matches!(text.as_str(), "254" | "255")
        {
            assert_eq!(
                checked.type_store.get(checked.types.expr_types[&expr]),
                Some(&etas_types::Type::Primitive(etas_types::PrimitiveType::U8))
            );
        }
    }
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::Number(crate::value::NumericValue::U8(255)))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn slice_view_retains_values_after_source_mutation_and_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow main() -> i32 {
    var values = [1, 2, 3, 4];
    let window = values[1, 4)[0, 2);
    values[1] = 99;
    checkpoint("slice");
    var sum = 0;
    for value in window limit Iterations(8) { sum = sum + value; }
    return sum;
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
    assert_eq!(result.value(), Some(&InterpValue::i32(5)));
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&InterpValue::i32(5)));
}

#[tokio::test(flavor = "current_thread")]
async fn lazy_iteration_continue_advances_and_nested_loops_keep_separate_cursors() {
    let checked = checked_project(
        r#"
module app.main;
flow main() -> i32 {
    var total = 0;
    for outer in [1, 2, 3] limit Iterations(10) {
        if outer == 1 { continue; }
        for inner in [10, 20] limit Iterations(10) {
            if inner == 10 { continue; }
            total = total + outer + inner;
        }
    }
    return total;
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
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(45)));
}

#[tokio::test(flavor = "current_thread")]
async fn lazy_iteration_checkpoint_retains_collection_version_and_validates_cursor() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow main() -> i32 {
    var values = [1, 2, 3];
    var sum = 0;
    for value in values limit Iterations(8) {
        values = [9, 9, 9];
        if value == 1 { checkpoint("loop"); }
        sum = sum + value;
    }
    return sum;
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
    assert_eq!(result.value(), Some(&InterpValue::i32(6)));
    let checkpoint = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
    let restored = checkpoint_from_json(&artifact, &checked).unwrap();
    let result = Interpreter
        .resume_checkpoint(&checked, &restored, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(6)));

    for (field, value) in [
        ("next_index", serde_json::json!(4)),
        ("iterations", serde_json::json!(0)),
        ("source", serde_json::Value::Null),
    ] {
        let mut bad = artifact.clone();
        let mut edits = 0;
        edit_for_loop(&mut bad, &mut |object| {
            object.insert(field.to_owned(), value.clone());
            edits += 1;
        });
        assert!(edits > 0, "must corrupt a real for-loop continuation");
        assert!(
            checkpoint_from_json(&bad, &checked).is_err(),
            "accepted invalid {field}"
        );
    }
    let mut old = artifact;
    old["schema"] = serde_json::json!("etas.cli.interpreter-checkpoint.v32");
    let error = checkpoint_from_json(&old, &checked).unwrap_err();
    assert!(
        error
            .message()
            .contains("expected `etas.cli.interpreter-checkpoint.v34`")
    );

    let mut bad = checkpoint.clone();
    let mut edits = 0;
    for frame in &mut bad.machine.frames {
        if let crate::orchestration::MachineFrameSnapshot::Continuation {
            continuation: crate::orchestration::ContinuationSnapshot::ForLoop { next_index, .. },
            ..
        } = frame
        {
            *next_index = 4;
            edits += 1;
        }
    }
    assert!(edits > 0);
    let rejected = Interpreter
        .resume_checkpoint(&checked, &bad, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(matches!(
        rejected.outcome,
        crate::api::RunOutcome::Failed(crate::api::RunFailure::RestoreRejected { .. })
    ));
    assert!(rejected.value().is_none());
    assert!(rejected.events.is_empty());
    assert!(rejected.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("checkpoint state validation failed")
    }));
}

fn edit_for_loop(
    value: &mut serde_json::Value,
    edit: &mut impl FnMut(&mut serde_json::Map<String, serde_json::Value>),
) {
    match value {
        serde_json::Value::Object(object) => {
            if object.get("kind").and_then(|kind| kind.as_str()) == Some("for_loop") {
                edit(object);
            }
            for value in object.values_mut() {
                edit_for_loop(value, edit);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                edit_for_loop(value, edit);
            }
        }
        _ => {}
    }
}

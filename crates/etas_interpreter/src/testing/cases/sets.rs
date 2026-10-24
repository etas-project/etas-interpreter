use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};
use crate::orchestration::{ContinuationSnapshot, MachineFrameSnapshot, ValueSnapshot};

#[tokio::test(flavor = "current_thread")]
async fn checked_sets_preserve_nested_values_nominal_types_and_empty_sets() {
    let checked = checked_project(
        r#"
module app.main;
type Id = i32;
flow main() -> bool {
    let empty: Set<i32> = #{};
    let ids = #{Id(1), Id(2), Id(1)};
    let arrays = #{[1, 2], [1, 2], [2, 1]};
    let nested = #{#{1, 2}, #{2, 1}, #{1}};
    var count_empty = 0;
    var count_ids = 0;
    var count_arrays = 0;
    var count_nested = 0;
    for value in empty limit Iterations(4) { count_empty = count_empty + 1; }
    for value in ids limit Iterations(4) { count_ids = count_ids + 1; }
    for value in arrays limit Iterations(4) { count_arrays = count_arrays + 1; }
    for value in nested limit Iterations(4) { count_nested = count_nested + 1; }
    return count_empty == 0 && count_ids == 2 && count_arrays == 2 && count_nested == 2
        && #{1, 2} == #{2, 1};
}
"#,
    );
    let host = FakeHost::new(HostServiceAvailability::default());
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
}

#[tokio::test(flavor = "current_thread")]
async fn set_literal_deduplicates_values_without_skipping_element_effects() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
flow element(value: i32, label: string) -> i32 {
    println(label);
    return value;
}
flow main() -> i32 {
    let values = #{element(2, "a"), element(1, "b"), element(2, "c"), element(1, "d")};
    var result = 0;
    for value in values limit Iterations(8) { result = result * 10 + value; }
    return result;
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
    assert_eq!(host.stdout_text(), "a\nb\nc\nd\n");
    assert_eq!(result.value(), Some(&InterpValue::i32(21)));
}

#[tokio::test(flavor = "current_thread")]
async fn set_iteration_resumes_in_order_and_rejects_duplicate_checkpoint_members() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow main() -> i32 {
    var values = #{2, 1, 2};
    let alias = values;
    var result = 0;
    for value in values limit Iterations(8) {
        values = #{9};
        if value == 2 { checkpoint("set-iteration"); }
        result = result * 10 + value;
    }
    if alias != #{1, 2} { return -1; }
    return result;
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
    assert_eq!(result.value(), Some(&InterpValue::i32(21)));
    assert_eq!(result.checkpoints.len(), 1);
    let saved = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&[], "main", saved).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    for checkpoint in [saved, &decoded] {
        let resumed = Interpreter
            .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::i32(21)));
    }

    let mut bad = artifact;
    fn duplicate_sets(value: &mut serde_json::Value) -> usize {
        match value {
            serde_json::Value::Object(fields) => {
                if fields.get("kind").and_then(serde_json::Value::as_str) == Some("set") {
                    let values = fields["values"].as_array().unwrap();
                    if let Some(first) = values.first().cloned() {
                        fields
                            .get_mut("values")
                            .unwrap()
                            .as_array_mut()
                            .unwrap()
                            .push(first);
                        return 1;
                    }
                }
                fields.values_mut().map(duplicate_sets).sum()
            }
            serde_json::Value::Array(values) => values.iter_mut().map(duplicate_sets).sum(),
            _ => 0,
        }
    }
    assert!(duplicate_sets(&mut bad) > 0);
    assert!(
        checkpoint_from_json(&bad, &checked)
            .unwrap_err()
            .message()
            .contains("duplicate set element")
    );

    let mut bad = saved.clone();
    let mut edits = 0;
    for frame in &mut bad.machine.frames {
        if let MachineFrameSnapshot::Continuation {
            continuation:
                ContinuationSnapshot::ForLoop {
                    source: Some(source),
                    ..
                },
            ..
        } = frame
        {
            let ValueSnapshot::Set(values) = source else {
                panic!("expected checked Set cursor");
            };
            let mut duplicate = values.iter().cloned().collect::<Vec<_>>();
            duplicate.push(duplicate[0].clone());
            *source = ValueSnapshot::Set(duplicate.into());
            edits += 1;
        }
    }
    assert_eq!(edits, 1);
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
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|d| d.message.contains("duplicate set element"))
    );
}

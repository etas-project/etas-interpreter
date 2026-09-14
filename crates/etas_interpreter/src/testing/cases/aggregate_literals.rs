use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};
use crate::orchestration::{ContinuationSnapshot as C, MachineFrameSnapshot as F};

#[tokio::test(flavor = "current_thread")]
async fn checked_literal_descriptors_preserve_order_and_reject_corrupt_resume_progress() {
    for (construction, expected, labels) in [
        (
            "[step(\"a\"), step(\"b\")]",
            "[\"a\", \"b\"]",
            vec!["a", "b"],
        ),
        (
            "[step(\"a\"); step(\"b\")]",
            "[\"a\"; \"b\"]",
            vec!["a", "b"],
        ),
        (
            "#{step(\"a\"), step(\"b\")}",
            "#{\"a\", \"b\"}",
            vec!["a", "b"],
        ),
        (
            "(step(\"a\"), step(\"b\"))",
            "(\"a\", \"b\")",
            vec!["a", "b"],
        ),
        (
            "{ first, second = step(\"a\"), third = step(\"b\") }",
            "{ first, second = \"a\", third = \"b\" }",
            vec!["a", "b"],
        ),
        (
            "{ step(\"a\") => step(\"b\"), step(\"c\") => step(\"d\") }",
            "{ \"a\" => \"b\", \"c\" => \"d\" }",
            vec!["a", "b", "c", "d"],
        ),
    ] {
        let checked = checked_project(&format!(
            r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
flow step(label: string) -> string {{ println(label); checkpoint(label); return label; }}
flow main() -> bool {{
    let first = "shorthand";
    let value = {construction};
    return value == {expected};
}}
"#
        ));
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
        assert!(
            result.diagnostics.is_empty(),
            "{construction}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(host.stdout_text(), format!("{}\n", labels.join("\n")));
        assert_eq!(result.checkpoints.len(), labels.len());
        let wrong_owner = checked
            .hir
            .exprs
            .iter()
            .find_map(|(id, data)| matches!(data, etas_hir::HirExpr::Literal(_)).then_some(id))
            .unwrap();
        for (index, checkpoint) in result.checkpoints.iter().enumerate() {
            let artifact = checkpoint_artifact_json(&[], "main", checkpoint).unwrap();
            let mut probe = artifact.clone();
            let continuation = literal_json(&mut probe).expect("suspended literal");
            for removed in ["fields", "entries", "exprs", "aggregate_kind"] {
                assert!(
                    continuation.get(removed).is_none(),
                    "copied descriptor {removed}"
                );
            }
            for invalid_progress in [false, true] {
                let mut corrupt = artifact.clone();
                let continuation = literal_json(&mut corrupt).unwrap();
                if invalid_progress {
                    let progress = if continuation.get("index").is_some() {
                        "index"
                    } else {
                        "next_index"
                    };
                    continuation[progress] = serde_json::json!(u32::MAX);
                } else {
                    continuation["expr"] = serde_json::json!(wrong_owner.0);
                }
                assert!(
                    checkpoint_from_json(&corrupt, &checked).is_err(),
                    "corrupt literal accepted"
                );

                let mut corrupt = checkpoint.clone();
                let continuation = corrupt
                    .machine
                    .frames
                    .iter_mut()
                    .find_map(|frame| match frame {
                        F::Block { continuation }
                        | F::Expr { continuation }
                        | F::Call { continuation, .. }
                        | F::Continuation { continuation }
                        | F::Handler { continuation }
                        | F::Retry { continuation } => literal_snapshot(continuation),
                        _ => None,
                    })
                    .unwrap();
                match continuation {
                    C::AggregateElement {
                        expr, next_index, ..
                    }
                    | C::RecordField {
                        expr, next_index, ..
                    } => {
                        if invalid_progress {
                            *next_index = usize::MAX;
                        } else {
                            *expr = wrong_owner;
                        }
                    }
                    C::MapKey { expr, index, .. } | C::MapValue { expr, index, .. } => {
                        if invalid_progress {
                            *index = usize::MAX;
                        } else {
                            *expr = wrong_owner;
                        }
                    }
                    _ => unreachable!(),
                }
                let rejected = Interpreter
                    .resume_checkpoint(&checked, &corrupt, &host, RunOptions::default())
                    .await
                    .unwrap();
                assert!(matches!(
                    rejected.outcome,
                    crate::api::RunOutcome::Failed(crate::api::RunFailure::RestoreRejected { .. })
                ));
                assert!(rejected.events.is_empty());
            }
            let mut corrupt = artifact.clone();
            let continuation = literal_json(&mut corrupt).unwrap();
            let extra = match continuation["kind"].as_str().unwrap() {
                "record_field" => serde_json::json!({"name":"extra", "value":{"kind":"unit"}}),
                "map_key" | "map_value" => {
                    serde_json::json!({"key":{"kind":"unit"}, "value":{"kind":"unit"}})
                }
                _ => serde_json::json!({"kind":"unit"}),
            };
            continuation["values"].as_array_mut().unwrap().push(extra);
            assert!(
                checkpoint_from_json(&corrupt, &checked).is_err(),
                "inconsistent evaluated count accepted"
            );

            let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
            let host = FakeHost::new(services);
            let resumed = Interpreter
                .resume_checkpoint(&checked, &decoded, &host, RunOptions::default())
                .await
                .unwrap();
            assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
            assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
            let remaining = &labels[index + 1..];
            assert_eq!(
                host.stdout_text(),
                if remaining.is_empty() {
                    String::new()
                } else {
                    format!("{}\n", remaining.join("\n"))
                }
            );
        }
    }
}

fn literal_json(value: &mut serde_json::Value) -> Option<&mut serde_json::Value> {
    if matches!(
        value.get("kind").and_then(serde_json::Value::as_str),
        Some("aggregate_element" | "record_field" | "map_key" | "map_value")
    ) {
        return Some(value);
    }
    match value {
        serde_json::Value::Object(object) => object.values_mut().find_map(literal_json),
        serde_json::Value::Array(array) => array.iter_mut().find_map(literal_json),
        _ => None,
    }
}

fn literal_snapshot(value: &mut C) -> Option<&mut C> {
    match value {
        C::AggregateElement { .. }
        | C::RecordField { .. }
        | C::MapKey { .. }
        | C::MapValue { .. } => Some(value),
        C::Chain { inner, outer } => literal_snapshot(inner).or_else(|| literal_snapshot(outer)),
        C::CallBoundary { outer } | C::HandlerDispatch { outer } => literal_snapshot(outer),
        C::HandleBoundary { inner, .. }
        | C::ScopedModelPolicy { inner, .. }
        | C::RestoreModelPolicy { inner, .. } => literal_snapshot(inner),
        _ => None,
    }
}

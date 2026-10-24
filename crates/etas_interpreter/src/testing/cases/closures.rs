use super::super::*;
use crate::{
    api::codec::{checkpoint_artifact_json, checkpoint_from_json},
    control::CallTarget,
};

#[tokio::test(flavor = "current_thread")]
async fn closure_does_not_retain_unused_payloads_or_project_slots() {
    let checked = checked_project(
        r#"
module app.main;
flow main(used: string, unused: string) -> () -> string {
    let extra = unused;
    return () => used;
}
"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![
                InterpValue::String("kept".into()),
                InterpValue::String("x".repeat(1_000_000).into()),
            ],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(InterpValue::Callable(CallTarget::Lambda { expr, captured })) = result.value() else {
        panic!("expected closure")
    };
    let locals = captured.sorted_locals();
    assert_eq!(locals.len(), 1);
    assert_eq!(checked.hir.symbols.get(locals[0].0).unwrap().name, "used");
    assert_eq!(locals[0].1, InterpValue::String("kept".into()));
    let plan = Interpreter.plan(&checked, Default::default()).plan.unwrap();
    let layout = plan.closures.get(*expr).unwrap();
    assert_eq!(layout.captures.len(), 1);
    assert_eq!(layout.slots.slot_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn nested_closure_captures_forwarded_bindings_and_record_shorthand() {
    let checked = checked_project(
        r#"
module app.main;
flow make(seed: i32) -> i32 -> i32 {
    let unused = "not captured";
    return (offset: i32) => {
        let nested = (value: i32) => {
            let record = { seed, offset };
            return record.seed + record.offset + value;
        };
        return nested(3);
    };
}
flow main() -> i32 { let f = make(7); return f(2); }
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
    assert_eq!(result.value(), Some(&InterpValue::i32(12)));
    let plan = Interpreter.plan(&checked, Default::default()).plan.unwrap();
    let mut names = checked
        .hir
        .exprs
        .iter()
        .filter(|(_, data)| matches!(data, etas_hir::HirExpr::Lambda { .. }))
        .map(|(expr, _)| {
            let mut names = plan
                .closures
                .get(expr)
                .unwrap()
                .captures
                .iter()
                .map(|s| checked.hir.symbols.get(*s).unwrap().name.clone())
                .collect::<Vec<_>>();
            names.sort();
            names
        })
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        vec![
            vec!["offset".to_owned(), "seed".to_owned()],
            vec!["seed".to_owned()]
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn shadowed_lambda_parameter_is_not_a_capture() {
    let checked = checked_project(
        r#"
module app.main;
flow make(value: i32) -> i32 -> i32 { return (value: i32) => value + 1; }
flow main() -> i32 { let f = make(99); return f(4); }
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
    assert_eq!(result.value(), Some(&InterpValue::i32(5)));
    let plan = Interpreter.plan(&checked, Default::default()).plan.unwrap();
    for (expr, data) in checked.hir.exprs.iter() {
        if matches!(data, etas_hir::HirExpr::Lambda { .. }) {
            assert!(plan.closures.get(expr).unwrap().captures.is_empty());
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn captured_bindings_survive_host_suspension_and_checkpoint_with_strict_layouts() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow make(seed: i32) -> i32 -> i32 {
    return (offset: i32) => {
        checkpoint("inside closure");
        return seed + offset;
    };
}
flow main() -> i32 {
    let unrelated = 100;
    let f = make(7);
    checkpoint("before closure");
    return f(2);
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
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.value(), Some(&InterpValue::i32(9)));
    assert_eq!(first.checkpoints.len(), 2);
    for checkpoint in &first.checkpoints {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
        let restored = checkpoint_from_json(&artifact, &checked).unwrap();
        let resumed = Interpreter
            .resume_checkpoint(&checked, &restored, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::i32(9)));
    }

    let unrelated = checked
        .hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "unrelated")
        .unwrap()
        .id;
    for inject in [false, true] {
        let mut bad =
            checkpoint_artifact_json(&["main.es".into()], "main", &first.checkpoints[0]).unwrap();
        let mut edits = 0;
        corrupt_captures(&mut bad, &mut |locals| {
            if inject {
                locals.push(serde_json::json!({"symbol": unrelated.0, "value": crate::api::codec::value_json(&InterpValue::i32(100))}));
            } else {
                locals.clear();
            }
            edits += 1;
        });
        assert!(edits > 0);
        let error = checkpoint_from_json(&bad, &checked).unwrap_err();
        assert!(
            error.message().contains(if inject {
                "outside its checked layout"
            } else {
                "missing a required capture"
            }),
            "{error:?}"
        );

        let mut bad = first.checkpoints[0].clone();
        let mut edits = 0;
        for machine_frame in &mut bad.machine.frames {
            use crate::orchestration::{
                CallTargetSnapshot, ContinuationSnapshot, MachineFrameSnapshot, ValueSnapshot,
            };
            let continuation = match machine_frame {
                MachineFrameSnapshot::Block { continuation }
                | MachineFrameSnapshot::Continuation { continuation } => continuation,
                _ => continue,
            };
            if let ContinuationSnapshot::ContinueBlock { frame, .. } = continuation {
                for (_, value) in std::rc::Rc::make_mut(&mut frame.locals) {
                    if let ValueSnapshot::Callable(CallTargetSnapshot::Lambda {
                        captured, ..
                    }) = value
                    {
                        if inject {
                            std::rc::Rc::make_mut(&mut captured.locals).push((
                                unrelated,
                                ValueSnapshot::capture(&InterpValue::i32(100)).unwrap(),
                            ));
                        } else {
                            std::rc::Rc::make_mut(&mut captured.locals).clear();
                        }
                        edits += 1;
                    }
                }
            }
        }
        assert!(edits > 0, "must corrupt a real captured frame");
        let rejected = Interpreter
            .resume_checkpoint(&checked, &bad, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(matches!(
            rejected.outcome,
            crate::api::RunOutcome::Failed(crate::api::RunFailure::RestoreRejected { .. })
        ));
        assert!(rejected.events.is_empty());
    }
}

fn corrupt_captures(
    value: &mut serde_json::Value,
    edit: &mut impl FnMut(&mut Vec<serde_json::Value>),
) {
    match value {
        serde_json::Value::Object(object) => {
            if object.get("kind").and_then(serde_json::Value::as_str) == Some("lambda") {
                if let Some(locals) = object
                    .get_mut("captured")
                    .and_then(|frame| frame.get_mut("locals"))
                    .and_then(serde_json::Value::as_array_mut)
                {
                    edit(locals);
                }
            }
            for value in object.values_mut() {
                corrupt_captures(value, edit);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                corrupt_captures(value, edit);
            }
        }
        _ => {}
    }
}

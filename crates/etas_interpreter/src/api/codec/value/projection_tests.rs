use super::*;
use crate::control::{CallTarget, Frame};

fn subprocess(flag: &str) -> bool {
    if std::env::var_os(flag).is_some() {
        return false;
    }
    let current = std::thread::current();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", current.name().unwrap(), "--nocapture"])
        .env(flag, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    true
}

#[test]
fn public_value_projection_keeps_deep_json_ownership() {
    if subprocess("ETAS_TEST_PUBLIC_VALUE_PROJECTION") {
        return;
    }
    let mut value = InterpValue::Bool(true);
    for _ in 0..30_000 {
        value = InterpValue::OptionSome(value.into());
    }
    let encoded = value_json(&value).unwrap();
    let duplicate = encoded.clone();
    assert_eq!(encoded, duplicate);
    assert_eq!(encoded.to_string().matches("option_some").count(), 30_000);
}

#[test]
fn runtime_callable_projection_traverses_captured_frame_graphs_without_recursion() {
    if subprocess("ETAS_TEST_CALLABLE_JSON_FRAME_GRAPH") {
        return;
    }
    let mut previous = None;
    for depth in [1000, 2000, 4000, 30_000] {
        let mut value = InterpValue::Bool(true);
        for _ in 0..depth {
            value = InterpValue::Callable(CallTarget::Lambda {
                expr: HirExprId(7),
                captured: Frame::from_snapshot(vec![(SymbolId(0), value)]).unwrap(),
            });
        }
        let (encoded, cost) = crate::testing::allocation::measure(|| value_json(&value).unwrap());
        let duplicate = encoded.clone();
        assert_eq!(encoded, duplicate);
        assert_eq!(
            encoded.to_string().matches("\"kind\":\"lambda\"").count(),
            depth
        );
        eprintln!("runtime callable JSON depth={depth}: {cost:?}");
        if let Some((size, bytes, count)) = previous {
            let ratio = depth.div_ceil(size);
            assert!(
                cost.bytes <= bytes * ratio + depth * 512,
                "nonlinear encoding: {cost:?}"
            );
            assert!(
                cost.count <= count * ratio + 64,
                "nonlinear encoding: {cost:?}"
            );
        }
        previous = Some((depth, cost.bytes, cost.count));
    }
}

#[test]
fn public_event_projection_keeps_deep_payload_ownership() {
    if subprocess("ETAS_TEST_PUBLIC_EVENT_PROJECTION") {
        return;
    }
    let mut value = InterpValue::Bool(true);
    for _ in 0..30_000 {
        value = InterpValue::OptionSome(value.into());
    }
    let event = WorkflowEvent::MessageCreated {
        id: "deep".into(),
        from: None,
        to: None,
        session: None,
        role: "user".into(),
        created_at: "step-1".into(),
        payload: Box::new(value),
        provenance: None,
    };
    let encoded = event_json(&event).unwrap();
    assert_eq!(encoded, encoded.clone());
    assert_eq!(encoded.to_string().matches("option_some").count(), 30_000);
}

#[test]
fn public_callable_projection_preserves_deep_wrapper_order() {
    if subprocess("ETAS_TEST_CALLABLE_JSON_WRAPPERS") {
        return;
    }
    let mut target = CallTarget::FlowItem(etas_hir::HirItemId(3));
    for index in 0..30_000 {
        target = match index % 3 {
            0 => CallTarget::Limited {
                target: target.into(),
                limits: vec![],
            },
            1 => CallTarget::Specialized {
                target: target.into(),
                type_bindings: vec![],
                effect_row_bindings: vec![],
            },
            _ => CallTarget::Composed(
                vec![target, CallTarget::ToolItem(etas_hir::HirItemId(5))].into(),
            ),
        };
    }
    let value = InterpValue::Callable(target);
    let encoded = value_json(&value).unwrap();
    assert_eq!(encoded, encoded.clone());
    let mut cursor = &encoded["target"];
    for index in (0..30_000).rev() {
        cursor = match index % 3 {
            0 => {
                assert_eq!(cursor["kind"], "limited");
                &cursor["target"]
            }
            1 => {
                assert_eq!(cursor["kind"], "specialized");
                &cursor["target"]
            }
            _ => {
                assert_eq!(cursor["kind"], "composed");
                assert_eq!(cursor["targets"][1], json!({"kind":"tool", "item":5}));
                &cursor["targets"][0]
            }
        };
    }
    assert_eq!(*cursor, json!({"kind":"flow", "item":3}));
    encoded.write_to(std::io::sink()).unwrap();
}

#[test]
fn runtime_projection_rejects_cycles_and_mutable_borrows_without_losing_errors() {
    let mut frame = Frame::from_snapshot(vec![(SymbolId(0), InterpValue::Bool(true))]).unwrap();
    let value = InterpValue::Callable(CallTarget::Lambda {
        expr: HirExprId(7),
        captured: frame.clone(),
    });
    frame.with_local_mut(SymbolId(0), |_| {
        assert!(
            value_json(&value)
                .unwrap_err()
                .message()
                .contains("mutably borrowed")
        );
    });
    assert!(frame.set(SymbolId(0), value.clone()));
    let (_, cost) = crate::testing::allocation::measure(|| {
        assert!(
            value_json(&value)
                .unwrap_err()
                .message()
                .contains("cyclic captured-frame")
        );
    });
    assert_eq!(cost.bytes, cost.released_bytes, "{cost:?}");
    // Repair the deliberately malformed native reference cycle after rejection.
    assert!(frame.set(SymbolId(0), InterpValue::Bool(true)));
    assert!(value_json(&value).is_ok());
}

#[test]
fn projected_captured_frames_are_independent_and_do_not_retain_runtime_owners() {
    use crate::plan::SlotLayoutTable;
    use std::sync::Arc;
    let layout = Arc::new(SlotLayoutTable::from_symbols(vec![
        SymbolId(1),
        SymbolId(0),
    ]));
    let weak = Arc::downgrade(&layout);
    let mut frame = Frame::new(layout);
    frame.insert(SymbolId(1), InterpValue::Bytes(vec![7; 1024].into()));
    frame.insert(SymbolId(0), InterpValue::String("original".into()));
    let value = InterpValue::Callable(CallTarget::Lambda {
        expr: HirExprId(7),
        captured: frame.clone(),
    });
    let encoded = value_json(&value).unwrap();
    assert_eq!(encoded["target"]["captured"]["locals"][0]["symbol"], 0);
    assert_eq!(encoded["target"]["captured"]["locals"][1]["symbol"], 1);
    assert!(frame.set(SymbolId(0), InterpValue::String("changed".into())));
    assert_eq!(
        encoded["target"]["captured"]["locals"][0]["value"]["value"],
        "original"
    );
    let duplicate = encoded.clone();
    drop((value, frame));
    assert!(
        weak.upgrade().is_none(),
        "JSON retained a live runtime frame"
    );
    assert_eq!(encoded, duplicate);
}

#[test]
fn collection_comparator_projection_uses_the_same_borrowed_frame_graph() {
    use crate::value::{
        ordered::{OrderedMapValue, OrderedSetValue},
        priority_queue::PriorityQueueValue,
    };
    let frame =
        Frame::from_snapshot(vec![(SymbolId(4), InterpValue::String("captured".into()))]).unwrap();
    let comparator = CallTarget::Lambda {
        expr: HirExprId(7),
        captured: frame,
    };
    let values = [
        InterpValue::OrderedMap(OrderedMapValue::new(Some(comparator.clone()))),
        InterpValue::OrderedSet(OrderedSetValue::new(Some(comparator.clone()))),
        InterpValue::PriorityQueue(PriorityQueueValue::new(Some(comparator))),
    ];
    for value in values {
        let encoded = value_json(&value).unwrap();
        assert_eq!(encoded["comparator"]["kind"], "callable");
        assert_eq!(
            encoded["comparator"]["target"]["captured"]["locals"][0],
            json!({"symbol":4,"value":{"kind":"string","value":"captured"}})
        );
    }
}

#[test]
fn host_trace_projection_preserves_public_payload_and_redacted_fields() {
    use etas_host::{HostTraceFieldSensitivity, HostTracePayload, HostValue};
    let mut payload = HostValue::String("leaf".into());
    for _ in 0..64 {
        payload = HostValue::List(vec![payload]);
    }
    let metadata = etas_host::HostTraceMetadata::from_payload(
        &HostTracePayload::new("test", "Test.audit")
            .with_field("public", payload, HostTraceFieldSensitivity::Public)
            .with_field(
                "secret",
                HostValue::String("never-persist".into()),
                HostTraceFieldSensitivity::Secret,
            ),
        &etas_host::HostTraceDigestKey::from_bytes([3; 32]),
    )
    .unwrap();
    let expected_digest = metadata.payload_digest.clone();
    let event = WorkflowEvent::HostTrace(etas_host::TraceEvent::ApprovalRequested {
        id: etas_host::HostRequestId(7),
        metadata,
        trace: etas_host::TraceContext::root(etas_host::TraceId(9)),
    });
    let encoded = event_json(&event).unwrap();
    assert_eq!(encoded["payload_digest"], expected_digest);
    assert_eq!(encoded["payload"][1]["sensitivity"], "secret");
    assert!(encoded["payload"][1]["value"].is_null());
    let mut cursor = &encoded["payload"][0]["value"];
    for _ in 0..64 {
        assert_eq!(cursor["kind"], "list");
        cursor = &cursor["values"][0];
    }
    assert_eq!(*cursor, json!({"kind":"string","value":"leaf"}));
    assert!(!encoded.to_string().contains("never-persist"));
}

#[tokio::test(flavor = "current_thread")]
async fn checked_source_returned_closure_graph_projects_without_losing_captures() {
    if subprocess("ETAS_TEST_CHECKED_RETURNED_CLOSURE_JSON") {
        return;
    }
    use crate::{
        Interpreter,
        api::{EntryPoint, RunOptions},
        testing::{FakeHost, project::checked_project},
    };
    let checked = checked_project(
        r#"
module app.main;
flow wrap(f: () -> i32 ![]) -> (() -> i32 ![]) { return () => f(); }
flow main() -> (() -> i32 ![]) {
    var f = () => 42;
    var n = 0;
    while n < 1000 limit Iterations(1001) { f = wrap(f); n = n + 1; }
    return f;
}
"#,
    );
    let host = FakeHost::new(crate::host::HostServiceAvailability::default());
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
    let encoded = value_json(result.value().unwrap()).unwrap();
    assert_eq!(encoded, encoded.clone());
    assert_eq!(
        encoded.to_string().matches("\"kind\":\"lambda\"").count(),
        1001
    );
    let report = run_report_json("run", &[], "main", &result).unwrap();
    assert_eq!(report["value"], encoded);
    report.write_to(std::io::sink()).unwrap();
    assert_eq!(host.console_call_count(), 0);
}

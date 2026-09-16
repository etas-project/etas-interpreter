use crate::{
    control::{CallTarget, Frame},
    orchestration::{CallTargetSnapshot, ValueSnapshot},
    testing::allocation::measure,
    value::*,
};
use etas_hir::{HirExprId, HirItemId, SymbolId};

fn lambda(frame: Frame) -> InterpValue {
    InterpValue::Callable(CallTarget::Lambda {
        expr: HirExprId(7),
        captured: frame,
    })
}

#[test]
fn distinct_frames_share_values_without_merging_frame_identity() {
    for count in [1000, 2000, 4000] {
        let payload = InterpValue::Array(vec![InterpValue::Bool(true); 128].into());
        let root = InterpValue::Array(
            (0..count)
                .map(|_| {
                    lambda(Frame::from_snapshot(vec![(SymbolId(0), payload.clone())]).unwrap())
                })
                .collect::<Vec<_>>()
                .into(),
        );
        let (saved, cost) = measure(|| ValueSnapshot::capture(&root).unwrap());
        eprintln!("distinct lambda frames={count}: {cost:?}");
        assert!(cost.count < count * 3, "repeated payload tables: {cost:?}");
        let ValueSnapshot::Array(values) = saved else {
            panic!("array")
        };
        let mut ids = std::collections::HashSet::new();
        let mut first_payload = None;
        for value in values.iter() {
            let ValueSnapshot::Callable(CallTargetSnapshot::Lambda { captured, .. }) = value else {
                panic!("lambda")
            };
            assert!(ids.insert(captured.id));
            let ValueSnapshot::Array(payload) = &captured.locals[0].1 else {
                panic!("payload")
            };
            if let Some(first) = first_payload {
                assert_eq!(first, payload.as_ptr());
            } else {
                first_payload = Some(payload.as_ptr());
            }
        }
    }
}

#[test]
fn separate_callable_values_reuse_shared_composition_storage() {
    for count in [1000, 2000, 4000] {
        let target = CallTarget::Composed(
            vec![
                CallTarget::FlowItem(HirItemId(1)),
                CallTarget::FlowItem(HirItemId(2)),
            ]
            .into(),
        );
        let root = InterpValue::Array(vec![InterpValue::Callable(target); count].into());
        let (saved, cost) = measure(|| ValueSnapshot::capture(&root).unwrap());
        eprintln!("shared callable tables={count}: {cost:?}");
        assert!(cost.count < 16, "repeated composition capture: {cost:?}");
        let ValueSnapshot::Array(values) = saved else {
            panic!("array")
        };
        let mut first = None;
        for value in values.iter() {
            let ValueSnapshot::Callable(CallTargetSnapshot::Composed(children)) = value else {
                panic!("composed")
            };
            if let Some(first) = first {
                assert_eq!(first, children.as_ptr());
            } else {
                first = Some(children.as_ptr());
            }
        }
    }
}

#[test]
fn new_capture_observes_frame_mutation_but_saved_frame_does_not() {
    let mut frame = Frame::from_snapshot(vec![(SymbolId(0), InterpValue::i32(1))]).unwrap();
    let root = InterpValue::Tuple(vec![lambda(frame.clone()), lambda(frame.clone())].into());
    let before = ValueSnapshot::capture(&root).unwrap();
    assert!(frame.set(SymbolId(0), InterpValue::i32(2)));
    let after = ValueSnapshot::capture(&root).unwrap();
    for (saved, expected) in [(before, 1), (after, 2)] {
        let ValueSnapshot::Tuple(values) = saved else {
            panic!("tuple")
        };
        for value in values.iter() {
            let ValueSnapshot::Callable(CallTargetSnapshot::Lambda { captured, .. }) = value else {
                panic!("lambda")
            };
            assert!(captured.locals[0].1.clone().restore().unwrap() == InterpValue::i32(expected));
        }
    }
}

#[test]
fn frame_cache_uses_storage_identity_not_untrusted_wire_id() {
    let first = Frame::from_snapshot(vec![(SymbolId(0), InterpValue::i32(1))]).unwrap();
    let mut second = Frame::from_snapshot(vec![(SymbolId(0), InterpValue::i32(2))]).unwrap();
    second.set_snapshot_id(first.snapshot_id()).unwrap();
    let root = InterpValue::Tuple(vec![lambda(first.clone()), lambda(second.clone())].into());
    let saved = ValueSnapshot::capture(&root).unwrap();
    assert!(
        saved
            .restore()
            .unwrap_err()
            .contains("conflicting definitions")
    );
}

#[test]
fn cyclic_frames_fail_closed_and_release_partial_capture() {
    const WORKER: &str = "ETAS_CYCLIC_FRAME_CAPTURE_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let thread = std::thread::current();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", thread.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let mut frame = Frame::from_snapshot(vec![
        (SymbolId(0), InterpValue::Unit),
        (
            SymbolId(1),
            InterpValue::Array(vec![InterpValue::Bool(true); 128].into()),
        ),
    ])
    .unwrap();
    assert!(frame.set(SymbolId(0), lambda(frame.clone())));
    let valid = Frame::from_snapshot(vec![(SymbolId(0), InterpValue::i32(42))]).unwrap();
    let root = InterpValue::Tuple(vec![lambda(valid.clone()), lambda(frame.clone())].into());
    let (_, cost) = measure(|| {
        assert!(
            ValueSnapshot::capture(&root)
                .unwrap_err()
                .contains("cyclic captured frame")
        );
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "failed capture leaked: {cost:?}"
    );
    // Break the deliberately malformed runtime reference cycle after the probe.
    assert!(frame.set(SymbolId(0), InterpValue::Unit));
    assert!(ValueSnapshot::capture(&root).is_ok());

    // A -> B -> C -> B exercises the indexed active set, not just its first slot.
    let mut a = Frame::from_snapshot(vec![(SymbolId(0), InterpValue::Unit)]).unwrap();
    let mut b = Frame::from_snapshot(vec![(SymbolId(0), InterpValue::Unit)]).unwrap();
    let mut c = Frame::from_snapshot(vec![(SymbolId(0), InterpValue::Unit)]).unwrap();
    assert!(a.set(SymbolId(0), lambda(b.clone())));
    assert!(b.set(SymbolId(0), lambda(c.clone())));
    assert!(c.set(SymbolId(0), lambda(b.clone())));
    let (_, cost) = measure(|| {
        assert!(
            ValueSnapshot::capture(&lambda(a.clone()))
                .unwrap_err()
                .contains("cyclic captured frame")
        );
    });
    assert_eq!(cost.bytes, cost.released_bytes);
    assert!(a.set(SymbolId(0), InterpValue::Unit));
    assert!(b.set(SymbolId(0), InterpValue::Unit));
    assert!(c.set(SymbolId(0), InterpValue::Unit));
}

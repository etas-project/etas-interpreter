use super::*;
use crate::{
    control::{CallTarget, Frame},
    orchestration::{CallTargetSnapshot, ValueSnapshot},
    testing::allocation::measure,
    value::{InterpValue, SharedValue},
};
use etas_hir::{HirExprId, SymbolId};

fn runtime_chain(depth: usize) -> SharedValue {
    let mut value = InterpValue::Bool(true);
    for _ in 0..depth {
        let captured = Frame::from_snapshot(vec![(SymbolId(0), value)]).unwrap();
        value = InterpValue::Callable(CallTarget::Lambda {
            expr: HirExprId(7),
            captured,
        });
    }
    value.into()
}

fn captured(value: &ValueSnapshot) -> crate::orchestration::LocalsSnapshot {
    let ValueSnapshot::Callable(CallTargetSnapshot::Lambda { captured, .. }) = value else {
        panic!("lambda");
    };
    captured.clone()
}

#[test]
fn frame_dependency_discovery_visits_shared_values_without_expanding_the_graph() {
    let frame = captured(&ValueSnapshot::capture(&runtime_chain(1)).unwrap());
    let mut value = ValueSnapshot::Callable(CallTargetSnapshot::Lambda {
        expr: HirExprId(7),
        captured: frame,
    });
    for _ in 0..1000 {
        value = ValueSnapshot::Array(vec![value.clone(), value].into());
    }
    assert!(value.walk_unique().count() < 5000);
    let mut dependencies = Vec::new();
    super::frame_dependencies::snapshot_frames(&value, &mut dependencies);
    assert_eq!(dependencies.len(), 1);
}

#[test]
fn captured_frame_graph_capture_restore_compare_and_release_are_stack_safe() {
    const WORKER: &str = "ETAS_TEST_CAPTURED_FRAME_GRAPH_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let current = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    for depth in [1000, 4000, 30000] {
        let runtime = runtime_chain(depth);
        let (saved, capture_cost) = measure(|| ValueSnapshot::capture(&runtime).unwrap());
        let independent = ValueSnapshot::capture(&runtime).unwrap();
        let (same, compare_cost) = measure(|| saved == independent);
        assert!(same);
        assert!(compare_cost.count < 64 && compare_cost.bytes < depth * 256 + 4096);
        assert_eq!(compare_cost.bytes, compare_cost.released_bytes);
        let (retained, clone_cost) = measure(|| saved.clone());
        assert_eq!(clone_cost.count, 0);
        let mut context = RestoreContext::default();
        let (restored, restore_cost) = measure(|| context.frame(captured(&saved)).unwrap());
        eprintln!(
            "captured frame graph depth={depth}: capture={capture_cost:?}; compare={compare_cost:?}; restore={restore_cost:?}; clone={clone_cost:?}"
        );
        let alias = context.frame(captured(&independent)).unwrap();
        let mut current = restored.clone();
        for index in 0..depth {
            let value = current.get(SymbolId(0)).unwrap();
            if index + 1 == depth {
                assert_eq!(value, InterpValue::Bool(true));
            } else {
                let InterpValue::Callable(CallTarget::Lambda { captured, .. }) = value else {
                    panic!("nested lambda");
                };
                current = captured;
            }
        }
        let mut invalid = independent;
        let mut cursor = &mut invalid;
        for _ in 0..depth {
            let ValueSnapshot::Callable(CallTargetSnapshot::Lambda { captured, .. }) = cursor
            else {
                panic!("snapshot lambda");
            };
            cursor = &mut std::rc::Rc::make_mut(&mut captured.locals)[0].1;
        }
        *cursor = ValueSnapshot::Bool(false);
        assert_ne!(saved, invalid);
        assert!(
            context
                .frame(captured(&invalid))
                .unwrap_err()
                .contains("conflicting definitions")
        );
        drop((
            alias, current, restored, context, invalid, saved, retained, runtime,
        ));
    }
}

#[test]
fn selection_predicate_frame_graph_restores_and_releases_without_recursion() {
    const WORKER: &str = "ETAS_TEST_SELECTION_PREDICATE_FRAME_GRAPH";
    if std::env::var_os(WORKER).is_none() {
        let current = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    for depth in [1000, 2000, 4000, 30000] {
        let mut runtime = InterpValue::Bool(true);
        for _ in 0..depth {
            runtime = InterpValue::MemorySelection {
                region_stable_id: "r".into(),
                path: vec![],
                key_type: etas_types::TypeId(0),
                value_type: etas_types::TypeId(0),
                kind: crate::value::MemorySelectionKind::Select,
                predicate: Some(Box::new(InterpValue::Callable(CallTarget::Lambda {
                    expr: HirExprId(7),
                    captured: Frame::from_snapshot(vec![(SymbolId(0), runtime)]).unwrap(),
                }))),
                limit: None,
            };
        }
        let (saved, capture_cost) = measure(|| ValueSnapshot::capture(&runtime).unwrap());
        let independent = ValueSnapshot::capture(&runtime).unwrap();
        assert_eq!(saved, independent);
        let (copy, clone_cost) = measure(|| saved.clone());
        assert!(
            clone_cost.count <= 2,
            "copied descendant graph: {clone_cost:?}"
        );
        let (restored, restore_cost) = measure(|| copy.restore().unwrap());
        assert!(capture_cost.count < depth * 64 + 64, "{capture_cost:?}");
        assert!(restore_cost.count < depth * 64 + 64, "{restore_cost:?}");
        eprintln!(
            "selection frame graph depth={depth}: capture={capture_cost:?}, clone={clone_cost:?}, restore={restore_cost:?}"
        );
        let mut cursor = restored.clone();
        for _ in 0..depth {
            let InterpValue::MemorySelection {
                predicate: Some(predicate),
                ..
            } = cursor
            else {
                panic!("selection")
            };
            let InterpValue::Callable(CallTarget::Lambda { captured, .. }) = *predicate else {
                panic!("lambda")
            };
            cursor = captured.get(SymbolId(0)).unwrap();
        }
        assert_eq!(cursor, InterpValue::Bool(true));
        drop((cursor, restored, independent, saved, runtime));
    }
}

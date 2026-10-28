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

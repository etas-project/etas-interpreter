use super::*;
use crate::testing::allocation::measure;
use etas_hir::HirItemId;

fn tree(depth: usize, mode: usize) -> CallTarget {
    let mut node = CallTarget::FlowItem(HirItemId(1));
    for _ in 0..depth {
        node = match mode {
            0 => CallTarget::Limited {
                target: node.into(),
                limits: vec![],
            },
            1 => CallTarget::Specialized {
                target: node.into(),
                type_bindings: vec![],
            },
            2 => CallTarget::Composed(vec![node, CallTarget::FlowItem(HirItemId(2))].into()),
            _ => CallTarget::Composed(vec![CallTarget::FlowItem(HirItemId(2)), node].into()),
        };
    }
    node
}

fn child_process(worker: &str) -> bool {
    if std::env::var_os(worker).is_some() {
        return false;
    }
    let current = std::thread::current();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", current.name().unwrap(), "--nocapture"])
        .env(worker, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "target subprocess: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    true
}

#[test]
fn runtime_call_target_clone_does_not_copy_its_descendant_graph() {
    if child_process("ETAS_TEST_RUNTIME_TARGET_CLONE") {
        return;
    }
    for depth in [32, 1000, 4000, 30_000] {
        for mode in 0..4 {
            let target = tree(depth, mode);
            let (cloned, cost) = measure(|| target.clone());
            eprintln!("runtime target clone depth={depth} mode={mode}: {cost:?}");
            assert_eq!(cost.count, 0, "copying descendant graph: {cost:?}");
            assert!(target == cloned);
            drop(cloned);
            drop(target);
        }
    }
}

#[test]
fn runtime_call_target_drop_is_stack_safe_without_a_test_cleanup_guard() {
    if child_process("ETAS_TEST_RUNTIME_TARGET_DROP") {
        return;
    }
    for depth in [1000, 4000, 30_000] {
        for mode in 0..4 {
            let target = tree(depth, mode);
            let (_, cost) = measure(|| drop(target));
            eprintln!("runtime target drop depth={depth} mode={mode}: {cost:?}");
            if mode < 2 {
                assert_eq!(cost.count, 0, "linear release allocates: {cost:?}");
            }
            assert!(cost.count <= 16, "per-edge scratch allocation: {cost:?}");
        }
    }
}

#[test]
fn runtime_call_target_cold_cost_and_wide_consuming_cost_are_explicit() {
    use super::call_target::{CallTargetChildren, CallTargetLink};
    let (boxed, old) = measure(|| Box::new(CallTarget::FlowItem(HirItemId(1))));
    let (linked, new) = measure(|| CallTargetLink::from(CallTarget::FlowItem(HirItemId(1))));
    eprintln!("cold owned edge={old:?}; shared edge={new:?}");
    assert_eq!(old.count, 1);
    assert_eq!(new.count, 1);
    assert_eq!(new.bytes, old.bytes + 2 * size_of::<usize>());
    drop((boxed, linked));

    for width in [1000, 4000, 30_000] {
        let build = || {
            (0..width)
                .map(|i| CallTarget::FlowItem(HirItemId(i as u32)))
                .collect::<Vec<_>>()
        };
        let (old, old_cost) = measure(build);
        let (values, cold_cost) = measure(|| CallTargetChildren::from(build()));
        eprintln!("wide cold width={width}: owned={old_cost:?}; shared={cold_cost:?}");
        assert_eq!(old_cost.count, 1);
        assert_eq!(cold_cost.count, 2);
        assert_eq!(
            cold_cost.bytes - old_cost.bytes,
            size_of::<Vec<CallTarget>>() + 2 * size_of::<usize>()
        );
        drop(old);
        let (alias, clone_cost) = measure(|| values.clone());
        assert_eq!(clone_cost.count, 0);
        let (shared_values, shared_cost) = measure(|| alias.into_values());
        assert_eq!(shared_cost.count, 1);
        assert_eq!(shared_cost.bytes, width * size_of::<CallTarget>());
        assert!(values.iter().eq(shared_values.iter()));
        drop(shared_values);
        let ptr = values.as_ptr();
        let (owned_values, unique_cost) = measure(|| values.into_values());
        assert_eq!(unique_cost.count, 0);
        assert_eq!(owned_values.as_ptr(), ptr);
    }
}

#[test]
fn runtime_call_target_cow_only_copies_edited_structure_and_keeps_frame_semantics() {
    use crate::{orchestration::ValueSnapshot, value::InterpValue};
    use etas_hir::{HirExprId, SymbolId};
    let mut original = CallTarget::Lambda {
        expr: HirExprId(7),
        captured: Frame::from_snapshot(vec![(SymbolId(9), InterpValue::i32(7))]).unwrap(),
    };
    for _ in 0..30_000 {
        original = CallTarget::Limited {
            target: original.into(),
            limits: vec![],
        };
    }
    let before = ValueSnapshot::capture(&InterpValue::Callable(original.clone())).unwrap();
    let retained = before.clone();
    let mut changed = original.clone();
    let mut cursor = &mut changed;
    for _ in 0..30_000 {
        let CallTarget::Limited { target, .. } = cursor else {
            panic!("limited")
        };
        cursor = target;
    }
    let CallTarget::Lambda { expr, captured } = cursor else {
        panic!("lambda")
    };
    *expr = HirExprId(8);
    assert!(captured.set(SymbolId(9), InterpValue::i32(9)));
    let mut original_leaf = &original;
    for _ in 0..30_000 {
        let CallTarget::Limited { target, .. } = original_leaf else {
            panic!("limited")
        };
        original_leaf = target;
    }
    let CallTarget::Lambda { expr, captured } = original_leaf else {
        panic!("lambda")
    };
    assert_eq!(*expr, HirExprId(7), "target edits must detach from aliases");
    assert_eq!(
        captured.get(SymbolId(9)),
        Some(InterpValue::i32(9)),
        "Frame cloning keeps existing alias semantics"
    );
    assert!(
        before == retained,
        "runtime changes must not change durable captures"
    );
    let after = ValueSnapshot::capture(&InterpValue::Callable(original)).unwrap();
    assert!(before != after);

    let original = CallTarget::Composed(vec![tree(30_000, 0), tree(30_000, 1)].into());
    let mut changed = original.clone();
    let (_, cost) = measure(|| {
        let CallTarget::Composed(children) = &mut changed else {
            panic!("composed")
        };
        children[1] = CallTarget::FlowItem(HirItemId(3));
    });
    assert_eq!(
        cost.count, 2,
        "COW copies a child table and owner, not the descendant graph"
    );
    let CallTarget::Composed(children) = &original else {
        panic!("composed")
    };
    assert!(matches!(children[1], CallTarget::Specialized { .. }));
}

#[test]
fn runtime_call_target_shared_dag_equality_and_release_are_bounded() {
    for depth in [1000, 4000, 30_000] {
        let build = |id| {
            let mut node = CallTarget::FlowItem(HirItemId(id));
            for _ in 0..depth {
                node = CallTarget::Composed(vec![node.clone(), node].into());
            }
            node
        };
        let left = build(1);
        let right = build(1);
        let different = build(2);
        let (same, cost) = measure(|| left == right);
        assert!(same);
        assert!(left != different);
        assert!(cost.count <= 64, "repeated DAG expansion: {cost:?}");
        assert!(
            cost.bytes <= depth * 384 + 4096,
            "unbounded comparison scratch: {cost:?}"
        );
        assert_eq!(cost.bytes, cost.released_bytes);
        eprintln!("runtime shared DAG compare depth={depth}: {cost:?}");
        let (_, released) = measure(|| drop(left));
        assert!(
            released.count <= 16,
            "per-node release allocation: {released:?}"
        );
    }
}

#[test]
fn shared_runtime_call_target_dags_capture_and_restore_without_expanding_aliases() {
    use crate::{
        eval::machine::snapshot::SnapshotValidator,
        orchestration::{
            CallTargetSnapshot, ContinuationSnapshot, MachineFrameSnapshot, MachineSnapshot,
            ValueSnapshot,
        },
        value::InterpValue,
    };
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    let validator = SnapshotValidator::new(
        &checked,
        &plan.slots,
        &plan.dispatch,
        &plan.closures,
        &limits,
    );
    let span = checked.hir.blocks.iter().next().unwrap().1.span;
    let validate = |target| {
        validator.validate_machine(&MachineSnapshot {
            frames: vec![MachineFrameSnapshot::Continuation {
                continuation: ContinuationSnapshot::ComposedCall {
                    remaining: vec![target],
                    span,
                },
            }],
        })
    };
    for depth in [1000, 4000, 30_000] {
        let mut target = CallTarget::FlowItem(checked.entry.unwrap());
        for _ in 0..depth {
            target = CallTarget::Composed(vec![target.clone(), target].into());
        }
        let runtime = InterpValue::Callable(target);
        let (snapshot, capture_cost) = measure(|| ValueSnapshot::capture(&runtime).unwrap());
        assert!(
            capture_cost.count <= 2 * depth + 64,
            "expanded runtime DAG: {capture_cost:?}"
        );
        let ValueSnapshot::Callable(saved) = &snapshot else {
            panic!("callable")
        };
        let (validated, validation_cost) = measure(|| validate(saved.clone()));
        validated.unwrap();
        assert!(
            validation_cost.count <= 64,
            "expanded validation DAG: {validation_cost:?}"
        );
        let mut changed = saved.clone();
        let mut leaf = &mut changed;
        for _ in 0..depth {
            let CallTargetSnapshot::Composed(children) = leaf else {
                panic!("composed")
            };
            leaf = &mut children[0];
        }
        *leaf = CallTargetSnapshot::FlowItem(HirItemId(u32::MAX));
        assert!(validate(changed).unwrap_err().contains("missing HIR item"));
        let invalid_binding = CallTargetSnapshot::Specialized {
            target: saved.clone().into(),
            type_bindings: vec![(String::new(), etas_types::TypeId(0))],
        };
        assert!(
            validate(invalid_binding)
                .unwrap_err()
                .contains("invalid or duplicate type parameter")
        );
        let (restored, restore_cost) = measure(|| snapshot.clone().restore().unwrap());
        assert!(
            restore_cost.count <= 3 * depth + 64,
            "expanded snapshot DAG: {restore_cost:?}"
        );
        assert!(restored == runtime);
        assert!(ValueSnapshot::capture(&restored).unwrap() == snapshot);
        eprintln!("runtime DAG depth={depth}: capture={capture_cost:?}; restore={restore_cost:?}");
        let (_, cost) = measure(|| {
            let copy = ValueSnapshot::capture(&runtime).unwrap();
            drop(copy.restore().unwrap());
        });
        assert_eq!(
            cost.bytes, cost.released_bytes,
            "retained capture/restore cache: {cost:?}"
        );
    }
}

#[test]
fn runtime_call_target_alias_retention_releases_every_allocation_at_last_owner() {
    for mode in 0..4 {
        let (_, cost) = measure(|| {
            let original = tree(30_000, mode);
            let alias = original.clone();
            drop(original);
            let retained = alias.clone();
            drop(alias);
            drop(retained);
        });
        assert_eq!(
            cost.bytes, cost.released_bytes,
            "mode={mode}: retained graph allocation: {cost:?}"
        );
    }
}

#[test]
fn runtime_call_target_clone_copies_root_metadata_but_not_descendant_metadata() {
    use crate::eval::limit::{RuntimeLimit, RuntimeLimitValue};
    let span = etas_core::Span::empty(etas_core::SourceId(7), etas_core::TextSize::ZERO);
    for depth in [1000, 4000, 30_000] {
        let mut value = CallTarget::FlowItem(HirItemId(1));
        for i in 0..depth {
            value = CallTarget::Specialized {
                target: CallTarget::Limited {
                    target: value.into(),
                    limits: vec![RuntimeLimit {
                        kind: etas_std::StdLimitKind::Attempts,
                        value: RuntimeLimitValue::Count(i as u64 + 1),
                        span,
                    }],
                }
                .into(),
                type_bindings: vec![("T".to_owned(), etas_types::TypeId(i as u32))],
            };
        }
        let (copy, cost) = measure(|| value.clone());
        assert_eq!(
            cost.count, 2,
            "only root binding table and its name: {cost:?}"
        );
        assert!(value == copy);
        eprintln!("nonempty target metadata clone depth={depth}: {cost:?}");
    }
}

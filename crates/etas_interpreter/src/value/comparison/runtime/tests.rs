use super::*;
use crate::{control::Frame, testing::allocation::measure, value::InterpValue as V};
use etas_hir::{HirExprId, SymbolId};

fn lambda(frame: Frame) -> V {
    V::Callable(CallTarget::Lambda {
        expr: HirExprId(7),
        captured: frame,
    })
}

fn equality_work(a: &V, b: &V) -> (bool, usize) {
    let context = Context::default();
    let result = compare(node::compare_node(a, b, &context));
    (result, context.visits.get())
}

#[test]
fn shared_callable_diamonds_are_compared_once_per_pair() {
    fn graph(depth: usize, leaf: bool) -> V {
        (0..depth).fold(V::Bool(leaf), |child, _| {
            lambda(
                Frame::from_snapshot(vec![(SymbolId(0), child.clone()), (SymbolId(1), child)])
                    .unwrap(),
            )
        })
    }
    for depth in [1000, 2000, 4000] {
        let a = graph(depth, true);
        let b = graph(depth, true);
        let different = graph(depth, false);
        let (equal, work) = equality_work(&a, &b);
        let (unequal, unequal_work) = equality_work(&a, &different);
        assert!(equal);
        assert!(!unequal);
        assert!(work <= 6 * depth + 2, "depth={depth}, visits={work}");
        assert!(
            unequal_work <= 4 * depth + 2,
            "depth={depth}, visits={unequal_work}"
        );
        eprintln!(
            "shared callable diamond depth={depth}: equal={work}, unequal={unequal_work} visits"
        );
    }
}

#[test]
fn frame_comparison_does_not_copy_wide_captured_payloads() {
    for sparse in [false, true] {
        for count in [1000, 2000, 4000] {
            let payload = V::String("x".repeat(4096).into());
            let make = || {
                if sparse {
                    Frame::from_snapshot(
                        (0..count).map(|n| (SymbolId(n), payload.clone())).collect(),
                    )
                    .unwrap()
                } else {
                    let mut frame = Frame::new(std::sync::Arc::new(
                        crate::plan::SlotLayoutTable::from_symbols(
                            (0..count).map(SymbolId).collect(),
                        ),
                    ));
                    for n in 0..count {
                        frame.insert(SymbolId(n), payload.clone());
                    }
                    frame
                }
            };
            let a = make();
            let b = make();
            let (equal, cost) = measure(|| a == b);
            assert!(equal);
            let symbol_bytes = if sparse {
                count as usize * std::mem::size_of::<SymbolId>()
            } else {
                0
            };
            assert!(
                cost.bytes <= symbol_bytes + 1024,
                "sparse={sparse}, n={count}: {cost:?}"
            );
            assert!(cost.count < 8, "{cost:?}");
            assert_eq!(
                cost.bytes, cost.released_bytes,
                "comparison retains no new payload/backing"
            );
            eprintln!(
                "frame sparse={sparse}, n={count}, logical payload={} bytes: {cost:?}",
                count as usize * 4096
            );
        }
    }
}

#[test]
fn cyclic_frame_comparison_rechecks_other_children_and_late_mismatches() {
    fn cycle(leaf: bool) -> Frame {
        let mut frame = Frame::from_snapshot(Vec::new()).unwrap();
        frame.insert(SymbolId(0), lambda(frame.clone()));
        frame.insert(SymbolId(1), V::Bool(leaf));
        frame
    }
    let mut a = cycle(true);
    let mut b = cycle(true);
    let mut c = cycle(false);
    assert!(a == b);
    assert!(a != c);
    assert!(a == b);
    // Break the intentionally cyclic test fixture, not the evaluator's traversal.
    for frame in [&mut a, &mut b, &mut c] {
        assert!(frame.set(SymbolId(0), V::Unit));
    }
}

#[test]
fn failed_set_candidates_do_not_reuse_cyclic_equality_proofs() {
    fn element(leaf: i32) -> (V, Frame) {
        let mut frame = Frame::from_snapshot(Vec::new()).unwrap();
        frame.insert(SymbolId(0), lambda(frame.clone()));
        frame.insert(SymbolId(1), V::i32(leaf));
        (lambda(frame.clone()), frame)
    }
    let (a0, mut af) = element(0);
    let (a1, mut a1f) = element(1);
    let (b0, mut bf) = element(0);
    let (b1, mut cf) = element(1);
    let (c1, mut c1f) = element(1);
    let (c2, mut c2f) = element(2);
    // Opaque callable fingerprints collide; matching must resume after a false
    // cyclic candidate without retaining its provisional equality proofs.
    let a = V::Set(vec![a0, a1].into());
    let b = V::Set(vec![b1, b0].into());
    let c = V::Set(vec![c1, c2].into());
    assert!(a == b);
    assert!(a != c);
    assert!(a == b);
    for frame in [&mut af, &mut a1f, &mut bf, &mut cf, &mut c1f, &mut c2f] {
        assert!(frame.set(SymbolId(0), V::Unit));
    }
}

#[test]
fn deep_comparator_targets_and_selection_predicates_share_the_worklist() {
    fn graph(depth: usize, leaf: bool) -> V {
        (0..depth).fold(V::Bool(leaf), |child, n| {
            let target = CallTarget::Lambda {
                expr: HirExprId(7),
                captured: Frame::from_snapshot(vec![(SymbolId(1), child)]).unwrap(),
            };
            match n % 5 {
                0 => V::MemorySelection {
                    region_stable_id: "test".into(),
                    path: vec!["rows".into()],
                    key_type: etas_types::TypeId(1),
                    value_type: etas_types::TypeId(2),
                    kind: crate::value::MemorySelectionKind::Select,
                    predicate: Some(Box::new(V::Callable(target))),
                    limit: None,
                },
                1 => V::Callable(CallTarget::Composed(
                    vec![CallTarget::Limited {
                        target: target.into(),
                        limits: Vec::new(),
                    }]
                    .into(),
                )),
                2 => V::OrderedMap(crate::value::OrderedMapValue::new(Some(target))),
                3 => V::OrderedSet(crate::value::OrderedSetValue::new(Some(target))),
                _ => V::PriorityQueue(crate::value::PriorityQueueValue::new(Some(target))),
            }
        })
    }
    let a = graph(4000, true);
    let b = graph(4000, true);
    let c = graph(4000, false);
    assert!(a == b);
    assert!(a != c);
}

#[test]
fn callable_metadata_and_nominal_identity_still_distinguish_values() {
    let make = |ty| {
        lambda(
            Frame::from_snapshot(vec![(
                SymbolId(1),
                V::Nominal {
                    ty,
                    value: V::Bool(true).into(),
                },
            )])
            .unwrap(),
        )
    };
    assert!(make(etas_types::TypeId(1)) != make(etas_types::TypeId(2)));
    let target = |bindings| CallTarget::Specialized {
        target: CallTarget::FlowItem(etas_hir::HirItemId(1)).into(),
        type_bindings: bindings,
        effect_row_bindings: Vec::new(),
    };
    assert!(
        target(vec![("T".into(), etas_types::TypeId(1))])
            != target(vec![("T".into(), etas_types::TypeId(2))])
    );
    assert!(
        CallTarget::FlowItem(etas_hir::HirItemId(1))
            != CallTarget::ToolItem(etas_hir::HirItemId(1))
    );
}

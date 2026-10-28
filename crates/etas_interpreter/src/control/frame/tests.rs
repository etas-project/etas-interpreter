use super::*;
use crate::testing::allocation::measure;

fn frame(restored: bool, count: u32) -> Frame {
    let mut frame = if restored {
        Frame::from_snapshot(Vec::new()).unwrap()
    } else {
        Frame::new(Arc::new(SlotLayoutTable::from_symbols(
            (0..count + 2).map(SymbolId).collect(),
        )))
    };
    for index in 0..count {
        frame.insert(SymbolId(index), InterpValue::i32(index as i32));
    }
    frame
}

#[test]
fn repeated_scope_snapshots_and_assignments_do_not_allocate() {
    for restored in [false, true] {
        for count in [1000, 2000, 4000] {
            let mut frame = frame(restored, count);
            let snapshot = frame.scope_symbols();
            let alias = frame.clone();
            let (_, cost) = measure(|| {
                for index in 0..1000 {
                    assert!(frame.set(SymbolId(0), InterpValue::i32(index)));
                    let current = alias.scope_symbols();
                    assert!(Rc::ptr_eq(&snapshot, &current));
                    frame.cleanup_to(&current);
                }
            });
            assert_eq!(cost.count, 0, "restored={restored}, n={count}: {cost:?}");
            assert_eq!(cost.bytes, 0, "{cost:?}");
            assert_eq!(frame.get(SymbolId(0)), Some(InterpValue::i32(999)));
            assert_eq!(frame.sorted_locals().len(), count as usize);
        }
    }
}

#[test]
fn scope_cache_invalidation_is_shared_but_old_snapshots_are_immutable() {
    for restored in [false, true] {
        let mut frame = frame(restored, 2);
        let outer = frame.scope_symbols();
        let mut alias = frame.clone();
        alias.insert(SymbolId(2), InterpValue::i32(2));
        let inner = frame.scope_symbols();
        assert!(!Rc::ptr_eq(&outer, &inner));
        assert!(!outer.contains(&SymbolId(2)));
        assert!(inner.contains(&SymbolId(2)));
        frame.with_local_mut(SymbolId(0), |value| *value = InterpValue::i32(42));
        frame.insert(SymbolId(1), InterpValue::i32(43));
        assert!(Rc::ptr_eq(&inner, &alias.scope_symbols()));
        alias.cleanup_to(&outer);
        assert_eq!(frame.get(SymbolId(2)), None);
        assert!(inner.contains(&SymbolId(2)));
        assert_eq!(*frame.scope_symbols(), *outer);
        assert_eq!(frame.get(SymbolId(0)), Some(InterpValue::i32(42)));
        assert_eq!(frame.get(SymbolId(1)), Some(InterpValue::i32(43)));
        alias.insert(SymbolId(2), InterpValue::i32(99));
        assert!(frame.scope_symbols().contains(&SymbolId(2)));
        frame.cleanup_to(&outer);
        assert_eq!(alias.get(SymbolId(2)), None);
    }
}

#[test]
fn installed_handler_slots_invalidate_only_when_bound() {
    let mut frame = frame(false, 1);
    let outer = frame.scope_symbols();
    let mut alias = frame.clone();
    alias.install_scope_layout(&SlotLayoutTable::from_symbols(vec![SymbolId(100)]));
    assert!(Rc::ptr_eq(&outer, &frame.scope_symbols()));
    alias.insert(SymbolId(100), InterpValue::i32(7));
    assert!(frame.scope_symbols().contains(&SymbolId(100)));
    frame.cleanup_to(&outer);
    assert_eq!(alias.get(SymbolId(100)), None);
}

#[test]
fn scope_cache_does_not_change_frame_equality() {
    let left = frame(false, 3);
    let right = frame(false, 3);
    left.scope_symbols();
    assert_eq!(left, right);
}

#[test]
fn equality_preserves_slot_layout_holes_sparse_storage_and_generic_bindings() {
    let dense = Frame::new(Arc::new(SlotLayoutTable::default()));
    let sparse = Frame::from_snapshot(Vec::new()).unwrap();
    assert!(dense != sparse);

    let a = frame(false, 2);
    let mut b = frame(false, 2);
    b.insert(SymbolId(2), InterpValue::Unit);
    assert!(a != b);
    let mut different_layout = Frame::new(Arc::new(SlotLayoutTable::from_symbols(vec![
        SymbolId(0),
        SymbolId(1),
        SymbolId(2),
        SymbolId(9),
    ])));
    different_layout.insert(SymbolId(0), InterpValue::i32(0));
    different_layout.insert(SymbolId(1), InterpValue::i32(1));
    assert!(a != different_layout);

    let layout = Arc::new(SlotLayoutTable::default());
    let a = Frame::with_type_bindings(
        layout.clone(),
        HashMap::from([("T".into(), etas_types::TypeId(1))]),
    );
    let b = Frame::with_type_bindings(layout, HashMap::from([("T".into(), etas_types::TypeId(2))]));
    assert!(a != b);

    let mut a = frame(false, 0);
    let mut b = frame(false, 0);
    a.install_scope_layout(&SlotLayoutTable::from_symbols(vec![SymbolId(10)]));
    b.install_scope_layout(&SlotLayoutTable::from_symbols(vec![SymbolId(11)]));
    a.insert(SymbolId(10), InterpValue::Bool(true));
    b.insert(SymbolId(11), InterpValue::Bool(true));
    assert!(a != b);
}

#[test]
fn equality_rejects_a_mutably_borrowed_capture_without_panicking() {
    for restored in [false, true] {
        let a = frame(restored, 2);
        let mut alias = a.clone();
        alias
            .with_local_mut(SymbolId(0), |_| assert!(a != a.clone()))
            .unwrap();
        assert!(a == alias);
    }
}

#[test]
fn equality_memo_does_not_merge_shared_slots_with_different_generic_environments() {
    let base = frame(false, 1);
    let other = frame(false, 1);
    let with_binding = |mut frame: Frame, ty| {
        frame.type_bindings = HashMap::from([("T".into(), etas_types::TypeId(ty))])
            .into_iter()
            .collect::<crate::control::GenericBindings>()
            .shared();
        InterpValue::Callable(crate::control::CallTarget::Lambda {
            expr: etas_hir::HirExprId(1),
            captured: frame,
        })
    };
    let a = InterpValue::Tuple(vec![with_binding(base.clone(), 1), with_binding(base, 2)].into());
    let b = InterpValue::Tuple(vec![with_binding(other.clone(), 1), with_binding(other, 3)].into());
    assert!(a != b);
}

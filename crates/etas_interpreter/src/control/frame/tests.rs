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

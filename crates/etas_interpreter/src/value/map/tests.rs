use super::*;

thread_local! {
    static KEY_COMPARISONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn record_comparison() {
    KEY_COMPARISONS.set(KEY_COMPARISONS.get() + 1);
}

#[test]
fn map_queries_use_linear_total_candidate_comparisons() {
    for count in [1000, 2000, 4000] {
        let key = |n| InterpValue::Tuple(vec![InterpValue::i32(n)].into());
        let map = MapValue::new((0..count).map(|n| (key(n), InterpValue::i32(n))).collect());
        KEY_COMPARISONS.set(0);
        for n in 0..count {
            assert!(map.contains_key(&key(n)));
            assert_eq!(map.get(&key(n)), Some(InterpValue::i32(n)));
            assert!(map.get(&key(n + count)).is_none());
        }
        let comparisons = KEY_COMPARISONS.get();
        assert!(
            comparisons <= count as usize * 3,
            "n={count}: {comparisons} comparisons"
        );
        eprintln!("map n={count}: contains/get/miss candidates={comparisons}");
    }
}

#[test]
fn unique_map_updates_preserve_buffer_and_index_with_linear_comparisons() {
    for count in [1000, 2000, 4000] {
        let entries: Vec<_> = (0..count)
            .map(|n| (InterpValue::i32(n), InterpValue::i32(n)))
            .collect();
        let pointer = entries.as_ptr();
        let mut map = MapValue::new(entries);
        assert_eq!(map.borrow().as_ptr(), pointer);
        assert!(map.0.borrow().index.get().is_none());
        assert!(map.contains_key(&InterpValue::i32(0)));
        let index = Rc::as_ptr(map.0.borrow().index.get().unwrap());
        KEY_COMPARISONS.set(0);
        let (_, allocations) = crate::testing::allocation::measure(|| {
            for n in 0..count {
                map.insert(InterpValue::i32(n), InterpValue::i32(n + 1));
                assert_eq!(map.get(&InterpValue::i32(n)), Some(InterpValue::i32(n + 1)));
            }
        });
        assert_eq!(allocations.count, 0, "n={count}: {allocations:?}");
        assert_eq!(map.borrow().as_ptr(), pointer);
        assert_eq!(Rc::as_ptr(map.0.borrow().index.get().unwrap()), index);
        let comparisons = KEY_COMPARISONS.get();
        assert!(
            comparisons <= count as usize * 3,
            "n={count}: {comparisons} comparisons"
        );
    }
}

#[test]
fn map_cow_shares_value_only_index_but_isolates_key_changes_and_restore() {
    use crate::{orchestration::ValueSnapshot, value::ArrayValue};
    let key = InterpValue::i32;
    let mut map = MapValue::new(vec![(
        key(1),
        InterpValue::Array(ArrayValue::new(vec![key(10)])),
    )]);
    assert!(map.contains_key(&key(1)));
    let index = Rc::as_ptr(map.0.borrow().index.get().unwrap());
    let alias = map.clone();
    let snapshot = ValueSnapshot::capture(&InterpValue::Map(alias.clone())).unwrap();
    {
        let mut value = map.value_mut(&key(1)).unwrap();
        let InterpValue::Array(values) = &mut *value else {
            panic!("array value");
        };
        values.borrow_mut()[0] = key(20);
    }
    assert_eq!(Rc::as_ptr(map.0.borrow().index.get().unwrap()), index);
    assert_eq!(
        alias.get(&key(1)),
        Some(InterpValue::Array(vec![key(10)].into()))
    );
    assert_eq!(
        map.get(&key(1)),
        Some(InterpValue::Array(vec![key(20)].into()))
    );
    map.insert(key(2), key(30));
    assert_ne!(Rc::as_ptr(map.0.borrow().index.get().unwrap()), index);
    assert_eq!(Rc::as_ptr(alias.0.borrow().index.get().unwrap()), index);
    assert!(!alias.contains_key(&key(2)));
    assert_eq!(map.get(&key(2)), Some(key(30)));

    map.borrow_mut().swap(0, 1);
    assert!(map.0.borrow().index.get().is_none());
    assert_eq!(map.get(&key(2)), Some(key(30)));
    map.borrow_mut()[0].0 = key(3);
    assert!(!map.contains_key(&key(2)));
    assert_eq!(map.get(&key(3)), Some(key(30)));
    map.borrow_mut().remove(1);
    assert!(!map.contains_key(&key(1)));
    assert!(alias.contains_key(&key(1)));

    let InterpValue::Map(restored) = snapshot.restore().unwrap() else {
        panic!("map snapshot");
    };
    assert!(
        restored.0.borrow().index.get().is_none(),
        "cache must not enter snapshots"
    );
    assert_eq!(restored.get(&key(1)), alias.get(&key(1)));
    assert!(!restored.contains_key(&key(3)));
}

#[test]
fn map_queries_preserve_compound_key_identity_and_nested_set_equivalence() {
    let wrap = |ty, members| InterpValue::Nominal {
        ty: etas_types::TypeId(ty),
        value: InterpValue::Set(members).into(),
    };
    let a = wrap(1, vec![InterpValue::i32(1), InterpValue::i32(2)].into());
    let b = wrap(1, vec![InterpValue::i32(2), InterpValue::i32(1)].into());
    let c = wrap(2, vec![InterpValue::i32(1), InterpValue::i32(2)].into());
    let mut map = MapValue::new(vec![
        (a, InterpValue::i32(10)),
        (c.clone(), InterpValue::i32(20)),
    ]);
    assert_eq!(map.get(&b), Some(InterpValue::i32(10)));
    map.insert(b, InterpValue::i32(30));
    assert_eq!(map.borrow().len(), 2);
    assert_eq!(map.get(&c), Some(InterpValue::i32(20)));
    assert!(!map.contains_key(&InterpValue::Set(
        vec![InterpValue::i32(1), InterpValue::i32(2)].into()
    )));
}

#[test]
fn shared_map_append_copies_entries_once_and_preserves_the_old_index() {
    for count in [1000, 2000, 4000] {
        let mut map = MapValue::new(
            (0..count)
                .map(|n| (InterpValue::i32(n), InterpValue::i32(n)))
                .collect(),
        );
        assert!(map.contains_key(&InterpValue::i32(0)));
        let alias = map.clone();
        let (_, allocations) = crate::testing::allocation::measure(|| {
            map.insert(InterpValue::i32(count), InterpValue::i32(count));
        });
        // New entry buffer + storage owner + index table + index owner. No
        // intermediate full entry buffer that must immediately be reallocated.
        assert_eq!(allocations.count, 4, "n={count}: {allocations:?}");
        assert_eq!(alias.borrow().len(), count as usize);
        assert!(!alias.contains_key(&InterpValue::i32(count)));
        assert_eq!(
            map.get(&InterpValue::i32(count)),
            Some(InterpValue::i32(count))
        );
        eprintln!("shared map append n={count}: {allocations:?}");
    }
}

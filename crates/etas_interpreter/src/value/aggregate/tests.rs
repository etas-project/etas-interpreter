use super::super::iteration::IterationSource;
use super::*;
use crate::{orchestration::ValueSnapshot, testing::allocation::measure};

#[test]
fn empty_array_pop_is_a_noop_even_when_backing_is_shared() {
    for capacity in [0, 4096] {
        let mut values = ArrayValue::new(Vec::with_capacity(capacity));
        let alias = values.clone();
        let (_, cost) = measure(|| assert_eq!(values.pop(), None));
        assert_eq!(cost.count, 0);
        assert!(Rc::ptr_eq(&values.0, &alias.0));
        assert_eq!(
            values.borrow().capacity(),
            capacity,
            "no-op keeps existing capacity, not compacted storage"
        );
    }
}

#[test]
fn shared_array_pop_copies_one_buffer_and_preserves_slice_and_snapshot_versions() {
    for count in [1000, 2000, 4000] {
        let mut values = ArrayValue::new(
            (0..count)
                .map(|_| InterpValue::String("payload".repeat(128).into()))
                .collect(),
        );
        let alias = values.clone();
        let slice = SliceValue::from_array(values.clone(), 0..count).unwrap();
        let snapshot = ValueSnapshot::capture(&InterpValue::Array(values.clone())).unwrap();
        let (popped, cost) = measure(|| values.pop().unwrap());
        assert_eq!(cost.count, 2, "one COW buffer and header: {cost:?}");
        assert!(
            cost.bytes <= count * std::mem::size_of::<InterpValue>() + 128,
            "copied nested payload: {cost:?}"
        );
        let (_, push_cost) = measure(|| values.push(popped));
        assert_eq!(
            push_cost.count, 0,
            "reuse capacity after pop: {push_cost:?}"
        );
        values.borrow_mut()[0] = InterpValue::String("changed".into());
        assert_eq!(alias.borrow().len(), count);
        assert_eq!(slice.borrow().len(), count);
        assert_eq!(slice.borrow()[0], alias.borrow()[0]);
        assert_ne!(values.borrow()[0], alias.borrow()[0]);
        assert_eq!(snapshot.restore().unwrap(), InterpValue::Array(alias));
        eprintln!("shared Array pop n={count}: {cost:?}");
    }
}

#[test]
fn concat_preserves_all_ownership_combinations_and_snapshot_versions() {
    for left_shared in [false, true] {
        for right_shared in [false, true] {
            for count in [1000, 2000, 4000] {
                let mut left = Vec::with_capacity(2 * count);
                left.extend((0..count).map(|i| InterpValue::i32(i as i32)));
                let left = ArrayValue::new(left);
                let right = ArrayValue::new(
                    (0..count)
                        .map(|i| InterpValue::i32((i + count) as i32))
                        .collect(),
                );
                let left_alias = left_shared.then(|| left.clone());
                let right_alias = right_shared.then(|| right.clone());
                let (mut joined, cost) = measure(|| left.concat(right));
                assert_eq!(
                    cost.count,
                    if left_shared { 2 } else { 0 },
                    "{left_shared}/{right_shared} {count}: {cost:?}"
                );
                let output_bytes = if left_shared {
                    2 * count * std::mem::size_of::<InterpValue>()
                } else {
                    0
                };
                assert!(
                    cost.bytes <= output_bytes + 128,
                    "extra input buffer: {cost:?}"
                );
                eprintln!(
                    "concat left_shared={left_shared} right_shared={right_shared} n={count}: {cost:?}"
                );
                assert_eq!(joined.borrow().len(), 2 * count);
                for (i, value) in joined.borrow().iter().enumerate() {
                    assert_eq!(*value, InterpValue::i32(i as i32));
                }
                let iterator = IterationSource::new(InterpValue::Array(joined.clone())).unwrap();
                let snapshot = ValueSnapshot::capture(&InterpValue::Array(joined.clone())).unwrap();
                joined.borrow_mut()[0] = InterpValue::i32(-1);
                assert_eq!(iterator.get(0).unwrap(), Some(InterpValue::i32(0)));
                let InterpValue::Array(restored) = snapshot.restore().unwrap() else {
                    panic!("array")
                };
                assert_eq!(restored.borrow()[0], InterpValue::i32(0));
                if let Some(left) = left_alias {
                    assert_eq!(left.borrow().len(), count);
                    assert_eq!(left.borrow()[0], InterpValue::i32(0));
                }
                if let Some(right) = right_alias {
                    assert_eq!(right.borrow().len(), count);
                    assert_eq!(right.borrow()[0], InterpValue::i32(count as i32));
                }
            }
        }
    }
}

#[test]
fn concat_self_and_empty_inputs_do_not_mutate_live_aliases() {
    let source = ArrayValue::new(vec![InterpValue::String("payload".repeat(1024).into())]);
    let pointer = source.borrow().as_ptr();
    for left_empty in [false, true] {
        let empty = ArrayValue::new(vec![]);
        let (joined, cost) = measure(|| {
            if left_empty {
                empty.concat(source.clone())
            } else {
                source.clone().concat(empty)
            }
        });
        assert_eq!(cost.count, 0);
        assert_eq!(joined.borrow().as_ptr(), pointer);
    }
    let mut doubled = source.clone().concat(source.clone());
    assert_eq!(doubled.borrow().len(), 2);
    assert_eq!(doubled.borrow()[0], doubled.borrow()[1]);
    doubled.borrow_mut()[0] = InterpValue::Unit;
    assert!(matches!(source.borrow()[0], InterpValue::String(_)));
    assert!(matches!(doubled.borrow()[1], InterpValue::String(_)));
}

#[test]
fn slice_retention_and_materialization_costs_follow_backing_ownership() {
    for count in [1000, 2000, 4000] {
        for shared in [false, true] {
            let values = ArrayValue::new(
                (0..count)
                    .map(|_| InterpValue::String("x".repeat(128).into()))
                    .collect(),
            );
            let capacity = values.borrow().capacity();
            let pointer = values.borrow().as_ptr();
            let payloads: [_; 3] = std::array::from_fn(|i| {
                let InterpValue::String(text) = &values.borrow()[i + 2] else {
                    panic!("string payload")
                };
                text.as_ptr()
            });
            let backing = Rc::downgrade(&values.0);
            let alias = shared.then(|| values.clone());
            let (view, view_cost) = measure(|| SliceValue::from_array(values, 2..5).unwrap());
            assert_eq!(view_cost.count, 0);
            assert_eq!(view.borrow().len(), 3);
            assert_eq!(view.backing.len(), count);
            assert_eq!(view.backing.capacity(), capacity);
            assert_eq!(backing.strong_count(), if shared { 2 } else { 1 });

            let (window, materialization) = measure(|| view.into_values());
            let output_capacity = if shared { 3 } else { capacity };
            assert_eq!(window.len(), 3);
            assert_eq!(window.capacity(), output_capacity);
            assert_eq!(materialization.count, usize::from(shared));
            assert_eq!(
                materialization.bytes,
                if shared {
                    3 * std::mem::size_of::<InterpValue>()
                } else {
                    0
                }
            );
            if !shared {
                assert_eq!(window.as_ptr(), pointer);
                assert!(materialization.released_bytes >= (count - 3) * 128);
            }
            for (value, pointer) in window.iter().zip(payloads) {
                let InterpValue::String(text) = value else {
                    panic!("string payload")
                };
                assert_eq!(text.as_ptr(), pointer, "selected payload was copied");
            }
            assert_eq!(backing.upgrade().is_some(), shared);
            let (_, alias_drop) = measure(|| drop(alias));
            assert!(backing.upgrade().is_none());
            if shared {
                assert!(
                    alias_drop.released_bytes
                        >= count * std::mem::size_of::<InterpValue>() + (count - 3) * 128
                );
            }
            let (_, output_drop) = measure(|| drop(window));
            assert!(
                output_drop.released_bytes
                    >= output_capacity * std::mem::size_of::<InterpValue>() + 3 * 128
            );
            eprintln!(
                "slice n={count} shared={shared}: view_backing={} header bytes + {} text bytes; view={view_cost:?}; materialize={materialization:?}; output_capacity={output_capacity}; alias_drop={alias_drop:?}; output_drop={output_drop:?}",
                capacity * std::mem::size_of::<InterpValue>(),
                count * 128
            );
        }
    }
}

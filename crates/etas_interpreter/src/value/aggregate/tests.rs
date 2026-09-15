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

use super::*;
use crate::{testing::allocation::measure, value::iteration::IterationSource};

fn strings(count: usize) -> ListValue {
    ListValue::new(
        (0..count)
            .map(|_| InterpValue::String("x".repeat(128).into()))
            .collect(),
    )
}

#[test]
fn cons_and_pop_share_tails_without_copying_payloads() {
    for count in [1000, 2000, 4000] {
        let original = strings(count);
        let mut list = original.clone();
        let head = InterpValue::String("head".repeat(128).into());
        let (_, allocations) = measure(|| list.push_front(head));
        assert_eq!(allocations.count, 1);
        assert!(Rc::ptr_eq(
            list.head.0.as_ref().unwrap().next.0.as_ref().unwrap(),
            original.head.0.as_ref().unwrap(),
        ));
        let (popped, allocations) = measure(|| list.pop_front());
        assert_eq!(allocations.count, 0, "unique cons head moves its payload");
        assert_eq!(popped, Some(InterpValue::String("head".repeat(128).into())));
        assert_eq!(list.len(), count);
        let (_, allocations) = measure(|| list.pop_front());
        assert_eq!(
            allocations.count, 0,
            "shared pop retains the shared head text"
        );
        assert_eq!(allocations.bytes, 0);
        assert!(Rc::ptr_eq(
            list.head.0.as_ref().unwrap(),
            original.head.0.as_ref().unwrap().next.0.as_ref().unwrap(),
        ));
        assert_eq!(original.len(), count);
        eprintln!(
            "List n={count}: cons=1 node, shared pop=0 payload bytes, retained tail={} nodes",
            list.len()
        );
    }
}

#[test]
fn indexed_write_copies_only_shared_prefix_and_keeps_nested_aliases() {
    for count in [1000, 2000, 4000] {
        let original = strings(count);
        let mut updated = original.clone();
        let (_, allocations) = measure(|| *updated.get_mut(2).unwrap() = InterpValue::Unit);
        assert_eq!(
            allocations.count, 3,
            "three nodes; their text payloads remain shared"
        );
        assert_eq!(updated.get(2), Some(&InterpValue::Unit));
        assert!(matches!(original.get(2), Some(InterpValue::String(_))));
        assert!(std::ptr::eq(
            original.get(3).unwrap(),
            updated.get(3).unwrap()
        ));
        let (_, allocations) = measure(|| *updated.get_mut(1).unwrap() = InterpValue::Unit);
        assert_eq!(allocations.count, 0, "already unique prefix");
        let (_, allocations) = measure(|| assert!(updated.get_mut(count).is_none()));
        assert_eq!(allocations.count, 0, "out of bounds must not copy anything");
    }
    let mut list = ListValue::new(vec![InterpValue::Array(vec![InterpValue::i32(1)].into())]);
    let alias = list.clone();
    let InterpValue::Array(array) = list.get_mut(0).unwrap() else {
        panic!("array")
    };
    array.borrow_mut()[0] = InterpValue::i32(9);
    let InterpValue::Array(array) = alias.get(0).unwrap() else {
        panic!("array")
    };
    assert_eq!(array.borrow()[0], InterpValue::i32(1));
}

#[test]
fn concatenation_reuses_unique_prefix_and_shares_right_tail() {
    for count in [1000, 2000, 4000] {
        let mut left = strings(count);
        let right = strings(count);
        let right_alias = right.clone();
        let (_, allocations) = measure(|| left.append(right));
        assert_eq!(allocations.count, 0);
        assert_eq!(left.len(), 2 * count);
        assert!(std::ptr::eq(
            left.get(count).unwrap(),
            right_alias.get(0).unwrap()
        ));
        let alias = left.clone();
        let (_, allocations) = measure(|| left.append(ListValue::default()));
        // Even an empty append must not walk/copy a shared prefix.
        assert_eq!(allocations.count, 0);
        assert_eq!(left, alias);
    }
}

#[test]
fn list_cursor_is_lazy_and_resumes_from_its_retained_tail() {
    for count in [1000, 2000, 4000] {
        let mut list = strings(count);
        let source_value = InterpValue::List(list.clone());
        let (source, setup) = measure(|| IterationSource::new(source_value).unwrap());
        assert_eq!(setup.count, 0);
        let (_, first) = measure(|| source.get(0).unwrap());
        assert_eq!(first.count, 0);
        assert_eq!(first.bytes, 0);
        *list.get_mut(0).unwrap() = InterpValue::Unit;
        NODE_VISITS.set(0);
        let (_, traversal) = measure(|| {
            for i in 1..count {
                assert!(matches!(
                    source.get(i).unwrap(),
                    Some(InterpValue::String(_))
                ));
            }
        });
        assert_eq!(traversal.count, 0);
        assert_eq!(traversal.bytes, 0);
        assert_eq!(
            NODE_VISITS.get(),
            2 * (count - 1),
            "one advance plus one head read per element"
        );
        assert!(source.get(count).unwrap().is_none());
        assert!(source.get(count + 1).unwrap().is_none());
        assert!(matches!(
            source.get(0).unwrap(),
            Some(InterpValue::String(_))
        ));
        let snapshot = crate::orchestration::ValueSnapshot::capture(source.value()).unwrap();
        let restored = IterationSource::new(snapshot.restore().unwrap()).unwrap();
        assert_eq!(
            restored.get(count - 1).unwrap(),
            source.get(count - 1).unwrap()
        );
    }
}

#[test]
fn long_list_drop_debug_and_equality_do_not_recurse_through_the_spine() {
    let count = 50_000;
    let mut list = ListValue::default();
    for _ in 0..count {
        list.push_front(InterpValue::Unit);
    }
    let mut alias = list.clone();
    assert_eq!(list, alias);
    assert_eq!(list.iter().count(), count);
    assert_eq!(format!("{list:?}").matches("Unit").count(), count);
    *alias.get_mut(count - 1).unwrap() = InterpValue::Bool(true);
    assert_ne!(list, alias);
    drop(list);
    drop(alias);
}

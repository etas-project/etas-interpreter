use super::*;
use crate::{orchestration::ValueSnapshot, testing::allocation::measure};

#[test]
fn construction_retains_first_occurrences_and_reuses_the_input_buffer() {
    for input in [vec![], vec![1], vec![2, 1, 2, 1], vec![1, 1, 1]] {
        let values: Vec<_> = input.iter().copied().map(InterpValue::i32).collect();
        let pointer = values.as_ptr();
        let set = SetValue::new(values);
        assert_eq!(set.borrow().as_ptr(), pointer);
        let mut expected = Vec::new();
        for value in input {
            if !expected.contains(&value) {
                expected.push(value);
            }
        }
        assert_eq!(
            set.borrow(),
            expected
                .into_iter()
                .map(InterpValue::i32)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn indexed_membership_preserves_aliases_and_uses_exact_equality_after_partitioning() {
    let first = InterpValue::Tuple(vec![InterpValue::i32(1)].into());
    let second = InterpValue::Tuple(vec![InterpValue::i32(2)].into());
    let mut set = SetValue::new(vec![first.clone(), second.clone(), first.clone()]);
    assert_eq!(set.borrow(), &[first.clone(), second.clone()]);
    let alias = set.clone();
    let pointer = set.borrow().as_ptr();
    assert!(!set.insert(first));
    assert_eq!(set.borrow().as_ptr(), pointer);
    assert!(set.insert(InterpValue::Tuple(vec![InterpValue::i32(3)].into())));
    assert_eq!(alias.borrow().len(), 2);
    assert_eq!(set.borrow().len(), 3);
    assert_eq!(
        alias,
        SetValue::new(vec![second.clone(), alias.borrow()[0].clone()])
    );
    let saved = ValueSnapshot::capture(&InterpValue::Set(alias.clone())).unwrap();
    set.clear();
    assert_eq!(saved.restore().unwrap(), InterpValue::Set(alias));
}

#[test]
fn numeric_and_nominal_identity_are_not_erased_by_the_index() {
    let wrap = |ty| InterpValue::Nominal {
        ty: etas_types::TypeId(ty),
        value: InterpValue::i32(1).into(),
    };
    let set = SetValue::new(vec![
        wrap(1),
        wrap(2),
        wrap(1),
        InterpValue::i32(1),
        InterpValue::u8(1),
    ]);
    assert_eq!(set.borrow().len(), 4);
    assert!(set.contains(&wrap(1)));
    assert!(!set.contains(&wrap(3)));
    assert!(set.contains(&InterpValue::u8(1)));
    assert!(!set.contains(&InterpValue::u16(1)));
}

#[test]
fn nested_set_membership_ignores_insertion_order_but_snapshot_identity_does_not() {
    let left = InterpValue::Set(vec![InterpValue::i32(2), InterpValue::i32(1)].into());
    let right = InterpValue::Set(vec![InterpValue::i32(1), InterpValue::i32(2)].into());
    assert_eq!(left, right);
    let a = ValueSnapshot::capture(&left).unwrap();
    let b = ValueSnapshot::capture(&right).unwrap();
    assert_ne!(
        a, b,
        "frame identity validation must preserve the saved iteration order"
    );
    let outer = SetValue::new(vec![left, right]);
    assert_eq!(outer.borrow().len(), 1);
    let malformed = ValueSnapshot::Set(vec![a, b].into());
    assert!(
        malformed
            .restore()
            .unwrap_err()
            .contains("duplicate set element")
    );
}

#[test]
fn scalar_set_queries_do_not_materialize_payloads() {
    for count in [1000, 2000, 4000] {
        let values: Vec<_> = (0..count)
            .map(|n| InterpValue::String(format!("{n:08}{}", "x".repeat(1024)).into()))
            .collect();
        let set = SetValue::new(values);
        let (_, allocations) = measure(|| {
            for value in set.borrow() {
                assert!(set.contains(value));
            }
        });
        assert_eq!(allocations.count, 0, "n={count}: {allocations:?}");
    }
}

#[test]
fn scalar_deduplication_and_membership_use_linear_candidate_comparisons() {
    for count in [1000, 2000, 4000] {
        let values: Vec<_> = (0..count)
            .flat_map(|n| [InterpValue::i32(n), InterpValue::i32(n)])
            .collect();
        let set = SetValue::new(values);
        assert_eq!(set.borrow().len(), count as usize);
        for n in 0..count {
            assert!(set.contains(&InterpValue::i32(n)));
        }
        let comparisons = set.0.index.comparison_count();
        assert!(
            comparisons <= count as usize * 3,
            "n={count}: {comparisons} comparisons"
        );
        eprintln!("set n={count}: dedup + membership equality candidates={comparisons}");
    }
}

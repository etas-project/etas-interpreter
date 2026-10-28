use super::super::tests::{reads, reset_reads};
use super::*;
use crate::{testing::allocation::measure, value::membership::MembershipIndex};

fn map(count: i32, different: Option<i32>) -> OrderedMapValue {
    let text: crate::value::StringValue = "x".repeat(4096).into();
    OrderedMapValue::try_from_entries(
        (0..count)
            .map(|n| {
                (
                    InterpValue::i32(n),
                    InterpValue::String(if different == Some(n) {
                        "different".into()
                    } else {
                        text.clone()
                    }),
                )
            })
            .collect(),
    )
    .unwrap()
}

fn set(count: i32, different: bool) -> OrderedSetValue {
    OrderedSetValue::try_from_values(
        (0..count)
            .map(|n| InterpValue::i32(n + i32::from(different)))
            .collect(),
    )
    .unwrap()
}

#[test]
fn cold_ordered_equality_does_not_materialize_flat_payload_views() {
    for count in [1000, 2000, 4000] {
        let a = map(count, None);
        let b = map(count, None);
        let x = InterpValue::OrderedMap(a.clone());
        let y = InterpValue::OrderedMap(b.clone());
        reset_reads();
        let (equal, cost) = measure(|| x == y);
        assert!(equal);
        assert_eq!(reads(), count as usize * 2);
        assert!(cost.bytes < 4096, "cold map n={count}: {cost:?}");
        assert!(a.0.entries.get().is_none() && b.0.entries.get().is_none());
        assert_eq!(cost.bytes, cost.released_bytes);
        eprintln!("cold ordered map equality n={count}: {cost:?}");

        let a = set(count, false);
        let b = set(count, false);
        reset_reads();
        let (equal, cost) =
            measure(|| InterpValue::OrderedSet(a.clone()) == InterpValue::OrderedSet(b.clone()));
        assert!(equal);
        assert_eq!(reads(), count as usize * 2);
        assert!(cost.bytes < 4096, "cold set n={count}: {cost:?}");
        assert!(a.0.keys.get().is_none() && b.0.keys.get().is_none());
        assert_eq!(cost.bytes, cost.released_bytes);
    }
}

#[test]
fn cold_ordered_hash_and_membership_do_not_initialize_snapshot_caches() {
    for count in [1000, 2000, 4000] {
        let index = MembershipIndex::default();
        let a = map(count, None);
        let value = InterpValue::OrderedMap(a.clone());
        reset_reads();
        let (_, cost) = measure(|| index.fingerprint(&value));
        assert_eq!(reads(), count as usize);
        assert!(cost.bytes < 2048, "cold map hash n={count}: {cost:?}");
        assert!(a.0.entries.get().is_none());
        assert_eq!(cost.bytes, cost.released_bytes);
        eprintln!("cold ordered map hash n={count}: {cost:?}");

        let saved = crate::value::SetValue::new(vec![value]);
        let query = map(count, None);
        let value = InterpValue::OrderedMap(query.clone());
        reset_reads();
        let (found, cost) = measure(|| saved.contains(&value));
        assert!(found);
        assert_eq!(reads(), count as usize * 3);
        assert!(cost.bytes < 8192, "cold membership n={count}: {cost:?}");
        assert!(query.0.entries.get().is_none());

        let a = set(count, false);
        reset_reads();
        let (_, cost) = measure(|| index.fingerprint(&InterpValue::OrderedSet(a.clone())));
        assert_eq!(reads(), count as usize);
        assert!(cost.bytes < 2048, "cold set hash n={count}: {cost:?}");
        assert!(a.0.keys.get().is_none());
    }
}

#[test]
fn cold_ordered_mismatches_do_not_populate_unvisited_views() {
    for count in [1000, 2000, 4000] {
        for different in [0, count - 1] {
            let a = map(count, None);
            let b = map(count, Some(different));
            reset_reads();
            let (unequal, cost) = measure(|| {
                InterpValue::OrderedMap(a.clone()) != InterpValue::OrderedMap(b.clone())
            });
            assert!(unequal);
            assert_eq!(reads(), (different as usize + 1) * 2);
            assert!(
                cost.bytes < 4096,
                "mismatch={different}, n={count}: {cost:?}"
            );
            assert!(a.0.entries.get().is_none() && b.0.entries.get().is_none());
        }
        let a = set(count, false);
        let b = set(count, true);
        reset_reads();
        assert!(InterpValue::OrderedSet(a.clone()) != InterpValue::OrderedSet(b.clone()));
        assert_eq!(reads(), 2);
        assert!(a.0.keys.get().is_none() && b.0.keys.get().is_none());
    }
}

#[test]
fn nested_ordered_comparison_and_hash_keep_the_traversal_iterative() {
    fn chain(depth: usize, leaf: bool) -> InterpValue {
        (0..depth).fold(InterpValue::Bool(leaf), |child, _| {
            InterpValue::OrderedMap(
                OrderedMapValue::try_from_entries(vec![(InterpValue::i32(0), child)]).unwrap(),
            )
        })
    }
    let a = chain(30_000, true);
    let b = chain(30_000, true);
    let c = chain(30_000, false);
    assert!(a == b);
    assert!(a != c);
    let index = MembershipIndex::default();
    assert_eq!(index.fingerprint(&a), index.fingerprint(&b));
    assert_ne!(index.fingerprint(&a), index.fingerprint(&c));
}

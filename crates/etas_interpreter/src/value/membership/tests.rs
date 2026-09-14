use std::hash::Hasher;

use super::{MembershipIndex, MembershipValue};
use crate::{
    orchestration::ValueSnapshot,
    testing::allocation::measure,
    value::{InterpValue as V, RecordValue, SetValue},
};

#[test]
fn hash_collisions_always_require_exact_membership_equality() {
    #[derive(PartialEq)]
    struct Collision(u32);
    impl MembershipValue for Collision {
        fn hash_partition(&self, _: &mut (impl Hasher + Clone)) {}
    }
    let values = [Collision(1), Collision(2)];
    let index = MembershipIndex::require_unique(&values).unwrap();
    assert_eq!(index.fingerprint(&values[0]), index.fingerprint(&values[1]));
    assert!(index.contains(&values, &Collision(2), index.fingerprint(&Collision(2))));
    assert!(!index.contains(&values, &Collision(3), index.fingerprint(&Collision(3))));
    assert!(MembershipIndex::require_unique(&[Collision(1), Collision(2), Collision(1)]).is_err());
}

#[test]
fn aggregate_fingerprints_respect_nested_set_equality() {
    fn wrap(child: V) -> Vec<V> {
        vec![
            V::Tuple(vec![child.clone()].into()),
            V::Array(vec![child.clone()].into()),
            V::List(vec![child.clone()].into()),
            V::Slice(vec![child.clone()].into()),
            V::Deque(vec![child.clone()].into()),
            V::Queue(vec![child.clone()].into()),
            V::Stack(vec![child.clone()].into()),
            V::Map(vec![(V::i32(0), child.clone())].into()),
            V::OrderedMap(vec![(V::i32(0), child.clone())].into()),
            V::PriorityQueue(vec![(V::i32(0), child.clone())].into()),
            V::Set(vec![child.clone()].into()),
            V::OrderedSet(vec![child.clone()].into()),
            V::Record(RecordValue::new(vec![("member".into(), child.clone())])),
            V::Variant {
                name: "Node".into(),
                fields: vec![child.clone()].into(),
            },
            V::Nominal {
                ty: etas_types::TypeId(123),
                value: child.clone().into(),
            },
            V::OptionSome(child.into()),
        ]
    }
    let a = V::Set(vec![V::i32(1), V::i32(2)].into());
    let b = V::Set(vec![V::i32(2), V::i32(1)].into());
    let index = MembershipIndex::default();
    for (a, b) in wrap(a).into_iter().zip(wrap(b)) {
        assert_eq!(a, b);
        assert_eq!(index.fingerprint(&a), index.fingerprint(&b));
        let sa = ValueSnapshot::capture(&a).unwrap();
        let sb = ValueSnapshot::capture(&b).unwrap();
        assert_ne!(sa, sb, "snapshot wire equality must retain iteration order");
        assert_eq!(index.fingerprint(&sa), index.fingerprint(&sb));
        assert!(MembershipIndex::require_unique(&[sa, sb]).is_err());
        assert_eq!(SetValue::new(vec![a, b]).borrow().len(), 1);
    }
}

#[test]
fn flat_aggregate_hashing_borrows_payloads_and_snapshot_nodes() {
    let index = MembershipIndex::default();
    for count in [1000, 2000, 4000] {
        let items: Vec<_> = (0..count)
            .map(|n| V::String(format!("{n}{}", "x".repeat(1024)).into()))
            .collect();
        for value in [
            V::Tuple(items.clone().into()),
            V::Array(items.clone().into()),
            V::List(items.into()),
        ] {
            let saved = ValueSnapshot::capture(&value).unwrap();
            let (_, runtime) = measure(|| index.fingerprint(&value));
            let (_, snapshot) = measure(|| index.fingerprint(&saved));
            assert_eq!(runtime.count, 0, "runtime n={count}: {runtime:?}");
            assert_eq!(snapshot.count, 0, "snapshot n={count}: {snapshot:?}");
        }
    }
}

#[test]
fn structural_hashing_visits_deep_values_without_cloning_subtrees() {
    let index = MembershipIndex::default();
    let chain = |leaf| (0..30_000).fold(V::i32(leaf), |value, _| V::Tuple(vec![value].into()));
    let a = chain(1);
    let b = chain(2);
    let sa = ValueSnapshot::capture(&a).unwrap();
    let sb = ValueSnapshot::capture(&b).unwrap();
    let (ha, runtime) = measure(|| index.fingerprint(&a));
    let (hsa, snapshot) = measure(|| index.fingerprint(&sa));
    assert_ne!(ha, index.fingerprint(&b));
    assert_ne!(hsa, index.fingerprint(&sb));
    // Only the geometric work-stack growth allocates; no per-node clone/box.
    assert!(runtime.count < 32, "{runtime:?}");
    assert!(snapshot.count < 32, "{snapshot:?}");
}

#[test]
fn cow_key_mutations_do_not_invalidate_an_existing_membership_index() {
    let mut array = crate::value::ArrayValue::new(vec![V::i32(1)]);
    let mut record = RecordValue::new(vec![("items".into(), V::Array(array.clone()))]);
    let set = SetValue::new(vec![V::Record(record.clone())]);
    let saved = ValueSnapshot::capture(&V::Set(set.clone())).unwrap();

    array.borrow_mut()[0] = V::i32(2);
    record.borrow_mut()[0].1 = V::Array(array);
    assert!(!set.contains(&V::Record(record)));
    let original = V::Record(RecordValue::new(vec![(
        "items".into(),
        V::Array(vec![V::i32(1)].into()),
    )]));
    assert!(set.contains(&original));
    assert_eq!(saved.restore().unwrap(), V::Set(set));
}

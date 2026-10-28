use std::hash::Hasher;

use super::{MembershipIndex, MembershipValue};
use crate::{
    orchestration::ValueSnapshot,
    testing::allocation::measure,
    value::{InterpValue as V, RecordValue, SetValue},
};

#[test]
fn shared_aggregate_dag_fingerprints_visit_backings_not_logical_expansion() {
    let index = MembershipIndex::default();
    for depth in [20, 1000, 2000, 4000] {
        let value = (0..depth).fold(V::Bool(true), |child, _| {
            V::Tuple(vec![child.clone(), child].into())
        });
        super::runtime::reset_visits();
        let (_, cost) = measure(|| index.fingerprint(&value));
        let visits = super::runtime::visits();
        assert!(visits <= 4 * depth + 1, "depth={depth}, visits={visits}");
        assert!(cost.bytes <= 1024 * depth + 4096, "{cost:?}");
        assert_eq!(cost.bytes, cost.released_bytes);
        eprintln!("shared aggregate hash depth={depth}, visits={visits}, scratch={cost:?}");
    }
}

#[test]
fn shared_record_dag_fingerprints_preserve_projection_context_without_expansion() {
    let index = MembershipIndex::default();
    for depth in [20, 1000, 2000, 4000] {
        let value = (0..depth).fold(V::Bool(true), |child, _| {
            V::Record(RecordValue::new(vec![
                ("a".into(), child.clone()),
                ("b".into(), child),
            ]))
        });
        super::runtime::reset_visits();
        let (_, cost) = measure(|| index.fingerprint(&value));
        let visits = super::runtime::visits();
        assert!(visits <= 8 * depth + 1, "depth={depth}, visits={visits}");
        assert!(cost.bytes <= 2048 * depth + 4096, "{cost:?}");
        assert_eq!(cost.bytes, cost.released_bytes);
        eprintln!("shared record hash depth={depth}, visits={visits}, scratch={cost:?}");
    }
}

#[test]
fn aggregate_fingerprints_do_not_depend_on_physical_sharing_layout() {
    fn node(family: usize, a: V, b: V) -> V {
        match family {
            0 => V::Tuple(vec![a, b].into()),
            1 => V::Array(vec![a, b].into()),
            2 => V::Stack(vec![a, b].into()),
            3 => V::List(vec![a, b].into()),
            4 => V::Deque(vec![a, b].into()),
            5 => V::Queue(vec![a, b].into()),
            6 => V::Record(RecordValue::new(vec![("a".into(), a), ("b".into(), b)])),
            7 => V::Map(crate::value::MapValue::new(vec![
                (V::i32(0), a),
                (V::i32(1), b),
            ])),
            8 => V::Slice(crate::value::SliceValue::from(vec![a, b])),
            9 => V::OrderedMap(
                crate::value::OrderedMapValue::try_from_entries(vec![
                    (V::i32(0), a),
                    (V::i32(1), b),
                ])
                .unwrap(),
            ),
            10 => V::PriorityQueue(crate::value::PriorityQueueValue::from_entries(
                vec![(V::i32(0), a), (V::i32(1), b)],
                None,
            )),
            11 => V::Variant {
                name: "Branch".into(),
                fields: vec![a, b].into(),
            },
            _ => V::Nominal {
                ty: etas_types::TypeId(19),
                value: V::Tuple(vec![a, b].into()).into(),
            },
        }
    }
    fn graph(family: usize, depth: usize, shared: bool) -> V {
        if depth == 0 {
            return V::Bool(true);
        }
        let a = graph(family, depth - 1, shared);
        let b = if shared {
            a.clone()
        } else {
            graph(family, depth - 1, false)
        };
        node(family, a, b)
    }
    let index = MembershipIndex::default();
    for family in 0..13 {
        let shared = graph(family, 8, true);
        let expanded = graph(family, 8, false);
        assert!(shared == expanded, "family={family}");
        assert_eq!(
            index.fingerprint(&shared),
            index.fingerprint(&expanded),
            "family={family}"
        );
    }
}

#[test]
fn hash_memo_retains_nominal_variant_and_slice_identity() {
    let index = MembershipIndex::default();
    let fields = crate::value::SharedFields::from(vec![V::Bool(true)]);
    let value = V::Tuple(
        vec![
            V::Variant {
                name: "A".into(),
                fields: fields.clone(),
            },
            V::Variant {
                name: "B".into(),
                fields: fields.clone(),
            },
        ]
        .into(),
    );
    let expanded = V::Tuple(
        vec![
            V::Variant {
                name: "A".into(),
                fields: vec![V::Bool(true)].into(),
            },
            V::Variant {
                name: "B".into(),
                fields: vec![V::Bool(true)].into(),
            },
        ]
        .into(),
    );
    assert_eq!(index.fingerprint(&value), index.fingerprint(&expanded));
    let payload: crate::value::SharedValue = V::Tuple(fields.clone()).into();
    let value = V::Tuple(
        vec![
            V::Nominal {
                ty: etas_types::TypeId(1),
                value: payload.clone(),
            },
            V::Nominal {
                ty: etas_types::TypeId(2),
                value: payload,
            },
        ]
        .into(),
    );
    let expanded = V::Tuple(
        vec![
            V::Nominal {
                ty: etas_types::TypeId(1),
                value: V::Tuple(vec![V::Bool(true)].into()).into(),
            },
            V::Nominal {
                ty: etas_types::TypeId(2),
                value: V::Tuple(vec![V::Bool(true)].into()).into(),
            },
        ]
        .into(),
    );
    assert_eq!(index.fingerprint(&value), index.fingerprint(&expanded));
    let a = V::Nominal {
        ty: etas_types::TypeId(1),
        value: V::Tuple(fields.clone()).into(),
    };
    let b = V::Nominal {
        ty: etas_types::TypeId(2),
        value: V::Tuple(fields).into(),
    };
    assert_ne!(index.fingerprint(&a), index.fingerprint(&b));
    let array = crate::value::ArrayValue::new(vec![V::Bool(true), V::Bool(false)]);
    let a = V::Slice(crate::value::SliceValue::from_array(array.clone(), 0..1).unwrap());
    let b = V::Slice(crate::value::SliceValue::from_array(array, 1..2).unwrap());
    assert_ne!(index.fingerprint(&a), index.fingerprint(&b));
}

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
            V::OrderedMap(
                crate::value::OrderedMapValue::try_from_entries(vec![(V::i32(0), child.clone())])
                    .unwrap(),
            ),
            V::PriorityQueue(vec![(V::i32(0), child.clone())].into()),
            V::Set(vec![child.clone()].into()),
            V::OrderedSet(crate::value::OrderedSetValue::from_tree(
                crate::value::ordered::OrderedTree::from_pending_entries(vec![(
                    child.clone(),
                    V::Unit,
                )])
                .unwrap(),
                Some(crate::control::CallTarget::FlowItem(etas_hir::HirItemId(0))),
            )),
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

fn range(
    start: crate::value::NumericValue,
    end: crate::value::NumericValue,
    bounds: crate::value::RangeBounds,
) -> V {
    V::Range(crate::value::RangeValue {
        start: Box::new(V::Number(start)),
        end: Box::new(V::Number(end)),
        bounds,
    })
}

#[test]
fn range_membership_uses_endpoint_partitions_without_expanding_ranges() {
    use crate::value::{NumericValue as N, RangeBounds as B};
    let key = |n| range(N::U64(n), N::U64(u64::MAX), B::ClosedClosed);
    for count in [1000, 2000, 4000] {
        let runtime: Vec<_> = (0..count).map(|n| key(n as u64)).collect();
        let snapshots: Vec<_> = runtime
            .iter()
            .map(|v| ValueSnapshot::capture(v).unwrap())
            .collect();
        let index = MembershipIndex::require_unique(&runtime).unwrap();
        let snapshot_index = MembershipIndex::require_unique(&snapshots).unwrap();
        let mut query_allocations = 0;
        for n in 0..count {
            for (offset, present) in [(0, true), (count, false)] {
                let value = key((n + offset) as u64);
                let snapshot = ValueSnapshot::capture(&value).unwrap();
                let (_, allocation) = measure(|| {
                    assert_eq!(
                        index.contains(&runtime, &value, index.fingerprint(&value)),
                        present
                    );
                    assert_eq!(
                        snapshot_index.contains(
                            &snapshots,
                            &snapshot,
                            snapshot_index.fingerprint(&snapshot)
                        ),
                        present
                    );
                });
                query_allocations += allocation.count;
            }
        }
        eprintln!(
            "Range n={count}: runtime={}, snapshot={} candidates, query allocations={query_allocations}",
            index.comparison_count(),
            snapshot_index.comparison_count()
        );
        assert!(index.comparison_count() <= count * 2);
        assert!(snapshot_index.comparison_count() <= count * 2);
        assert_eq!(query_allocations, 0);
    }
}

#[test]
fn range_partitions_preserve_endpoint_type_and_boundary_identity() {
    use crate::value::{NumericValue as N, RangeBounds as B};
    let values = [
        range(N::I32(0), N::I32(10), B::ClosedOpen),
        range(N::I32(1), N::I32(10), B::ClosedOpen),
        range(N::I32(0), N::I32(11), B::ClosedOpen),
        range(N::U32(0), N::U32(10), B::ClosedOpen),
        range(N::I64(0), N::I64(10), B::ClosedOpen),
        range(N::I8(0), N::I8(10), B::ClosedOpen),
        range(N::I16(0), N::I16(10), B::ClosedOpen),
        range(N::I128(0), N::I128(10), B::ClosedOpen),
        range(N::ISize(0), N::ISize(10), B::ClosedOpen),
        range(N::U8(0), N::U8(10), B::ClosedOpen),
        range(N::U16(0), N::U16(10), B::ClosedOpen),
        range(N::U64(0), N::U64(10), B::ClosedOpen),
        range(N::U128(0), N::U128(10), B::ClosedOpen),
        range(N::USize(0), N::USize(10), B::ClosedOpen),
        range(N::I32(-1), N::I32(10), B::ClosedOpen),
        range(N::I32(0), N::I32(10), B::ClosedClosed),
        range(N::I32(0), N::I32(10), B::OpenClosed),
        range(N::I32(0), N::I32(10), B::OpenOpen),
    ];
    let index = MembershipIndex::require_unique(&values).unwrap();
    let snapshots: Vec<_> = values
        .iter()
        .map(|v| ValueSnapshot::capture(v).unwrap())
        .collect();
    let snapshot_index = MembershipIndex::require_unique(&snapshots).unwrap();
    for (i, a) in values.iter().enumerate() {
        assert_eq!(index.fingerprint(a), index.fingerprint(&a.clone()));
        for (j, b) in values.iter().enumerate() {
            assert_eq!(a == b, i == j);
            assert_eq!(index.fingerprint(a) == index.fingerprint(b), i == j);
            assert_eq!(
                snapshot_index.fingerprint(&snapshots[i])
                    == snapshot_index.fingerprint(&snapshots[j]),
                i == j
            );
        }
    }
    assert_eq!(
        SetValue::new(
            values
                .iter()
                .cloned()
                .chain(values.iter().cloned())
                .collect()
        )
        .borrow()
        .len(),
        values.len()
    );
}

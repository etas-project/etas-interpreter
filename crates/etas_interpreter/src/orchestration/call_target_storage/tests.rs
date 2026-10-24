use super::*;
use crate::testing::allocation::measure;
use etas_hir::HirItemId;

fn chain(depth: usize, mode: usize, leaf: u32) -> CallTargetSnapshotLink {
    let mut node: CallTargetSnapshotLink = CallTargetSnapshot::FlowItem(HirItemId(leaf)).into();
    for _ in 0..depth {
        node = match mode {
            0 => CallTargetSnapshot::Limited {
                target: node,
                limits: vec![],
            },
            1 => CallTargetSnapshot::Specialized {
                target: node,
                type_bindings: vec![],
            },
            2 => CallTargetSnapshot::Composed(
                (vec![
                    node.into_value(),
                    CallTargetSnapshot::FlowItem(HirItemId(2)),
                ])
                .into(),
            ),
            _ => CallTargetSnapshot::Composed(
                (vec![
                    CallTargetSnapshot::FlowItem(HirItemId(2)),
                    node.into_value(),
                ])
                .into(),
            ),
        }
        .into();
    }
    node
}

#[test]
fn deep_call_target_links_clone_compare_and_release_without_recursive_edges() {
    for depth in [1000, 4000, 30_000] {
        for mode in 0..4 {
            let a = chain(depth, mode, 1);
            let b = chain(depth, mode, 1);
            let c = chain(depth, mode, 3);
            let (same, equal_cost) = measure(|| a == b);
            assert!(same);
            assert!(a != c);
            assert!(equal_cost.count < 32);
            assert!(equal_cost.bytes < depth * 192 + 4096);
            assert_eq!(equal_cost.bytes, equal_cost.released_bytes);
            let (retained, clone_cost) = measure(|| a.clone());
            assert_eq!(clone_cost.count, 0);
            let (_, shared_release) = measure(|| drop(a));
            assert_eq!(shared_release.count, 0);
            let (_, release) = measure(|| drop(retained));
            eprintln!(
                "call target ownership depth={depth} mode={mode}: compare={equal_cost:?} release={release:?}"
            );
            assert!(release.count < 32);
            assert!(release.bytes < depth * 192 + 4096);
            if mode < 2 {
                assert_eq!(equal_cost.count, 0);
                assert_eq!(release.count, 0);
            }
        }
    }
}

#[test]
fn shared_target_release_preserves_aliases_and_releases_every_unique_node() {
    for depth in [1000, 4000, 30_000] {
        let mut node: CallTargetSnapshotLink = CallTargetSnapshot::FlowItem(HirItemId(1)).into();
        let mut weak = Vec::new();
        for index in 0..depth {
            weak.push(Rc::downgrade(node.0.as_ref().unwrap()));
            node = match index % 3 {
                0 => CallTargetSnapshot::Limited {
                    target: node,
                    limits: vec![],
                },
                1 => CallTargetSnapshot::Specialized {
                    target: node,
                    type_bindings: vec![],
                },
                _ => CallTargetSnapshot::Composed(
                    (vec![
                        CallTargetSnapshot::Limited {
                            target: node.clone(),
                            limits: vec![],
                        },
                        CallTargetSnapshot::Limited {
                            target: node,
                            limits: vec![],
                        },
                    ])
                    .into(),
                ),
            }
            .into();
        }
        let retained = node.clone();
        drop(node);
        assert!(weak.iter().all(|node| node.upgrade().is_some()));
        drop(retained);
        assert!(weak.iter().all(|node| node.upgrade().is_none()));
    }
}

#[test]
fn changing_cloned_target_path_does_not_mutate_saved_target() {
    let original = chain(30_000, 0, 1);
    let mut changed = original.clone();
    let mut cursor = &mut *changed;
    for _ in 0..30_000 {
        let CallTargetSnapshot::Limited { target, .. } = cursor else {
            panic!("limited target")
        };
        cursor = target;
    }
    *cursor = CallTargetSnapshot::FlowItem(HirItemId(7));
    assert!(original != changed);
    let unchanged = chain(30_000, 0, 1);
    assert!(original == unchanged);
    drop(changed);
    assert!(original == unchanged);
}

#[test]
fn consuming_shared_and_unique_target_links_only_clones_the_current_header() {
    for shared in [false, true] {
        let node = chain(30_000, 0, 1);
        let retained = shared.then(|| node.clone());
        let (value, cost) = measure(|| node.into_value());
        assert_eq!(cost.count, 0);
        assert!(matches!(value, CallTargetSnapshot::Limited { .. }));
        drop(value);
        if let Some(retained) = retained {
            assert!(matches!(*retained, CallTargetSnapshot::Limited { .. }));
        }
    }
}

#[test]
fn wide_target_link_ownership_does_not_allocate_per_child_during_release_or_compare() {
    for width in [1000, 4000, 30_000] {
        let build = || -> CallTargetSnapshotLink {
            CallTargetSnapshot::Composed(
                ((0..width)
                    .map(|i| CallTargetSnapshot::FlowItem(HirItemId(i)))
                    .collect::<Vec<_>>())
                .into(),
            )
            .into()
        };
        let first = build();
        let second = build();
        let (same, cost) = measure(|| first == second);
        assert!(same);
        assert!(
            cost.count <= 1 && cost.bytes <= 256,
            "wide comparison: {cost:?}"
        );
        let (_, cost) = measure(|| drop(first));
        assert!(
            cost.count <= 1 && cost.bytes <= 256,
            "wide release: {cost:?}"
        );
    }
}

#[test]
fn call_target_link_cold_owner_cost_is_explicit() {
    let (boxed, old) = measure(|| Box::new(CallTargetSnapshot::FlowItem(HirItemId(1))));
    let (link, cost) =
        measure(|| CallTargetSnapshotLink::from(CallTargetSnapshot::FlowItem(HirItemId(1))));
    assert_eq!(old.count, 1);
    assert_eq!(cost.count, 1);
    assert_eq!(cost.bytes, old.bytes + 2 * size_of::<usize>());
    eprintln!("call target cold edge: Box={old:?} shared={cost:?}");
    drop((boxed, link));
}

#[test]
fn repeated_shared_target_subgraphs_are_compared_once_per_node_pair() {
    for depth in [1000, 4000, 30_000] {
        let graph = |id| {
            let mut node = CallTargetSnapshot::FlowItem(HirItemId(id));
            for _ in 0..depth {
                node = CallTargetSnapshot::Composed((vec![node.clone(), node]).into());
            }
            node
        };
        let a = graph(1);
        let b = graph(1);
        let c = graph(2);
        let (same, cost) = measure(|| a == b);
        assert!(same);
        assert!(a != c);
        eprintln!("shared call target graph depth={depth}: {cost:?}");
        assert!(cost.count < 64 && cost.bytes < depth * 384 + 8192);
        assert_eq!(cost.bytes, cost.released_bytes);
    }
}

#[test]
fn composed_target_cold_and_retained_copy_costs_are_reported_separately() {
    for count in [1usize, 1000, 4000] {
        let (inline, old) = measure(|| {
            (0..count)
                .map(|i| CallTargetSnapshot::FlowItem(HirItemId(i as u32)))
                .collect::<Vec<_>>()
        });
        let (target, cold) = measure(|| {
            CallTargetSnapshot::Composed(
                ((0..count)
                    .map(|i| CallTargetSnapshot::FlowItem(HirItemId(i as u32)))
                    .collect::<Vec<_>>())
                .into(),
            )
        });
        let (retained, copy) = measure(|| target.clone());
        assert_eq!(old.count, 1);
        assert_eq!(cold.count, 2);
        assert_eq!(
            cold.bytes,
            old.bytes + 2 * size_of::<usize>() + size_of::<Vec<CallTargetSnapshot>>()
        );
        assert_eq!(copy.count, 0);
        assert_eq!(copy.bytes, 0);
        eprintln!(
            "composed target width={count}: old inline={old:?}; cold links={cold:?}; retained copy={copy:?}"
        );
        drop((inline, target, retained));
    }
}

#[test]
fn changing_a_shared_composed_table_does_not_mutate_saved_children() {
    let saved = CallTargetSnapshot::Composed(
        vec![
            chain(30_000, 0, 1).into_value(),
            CallTargetSnapshot::FlowItem(HirItemId(2)),
        ]
        .into(),
    );
    let mut changed = saved.clone();
    let CallTargetSnapshot::Composed(children) = &mut changed else {
        panic!("composed target")
    };
    let (_, cost) = measure(|| children[1] = CallTargetSnapshot::FlowItem(HirItemId(3)));
    assert_eq!(
        cost.count, 2,
        "copy only the child table and Rc header: {cost:?}"
    );
    assert_eq!(
        cost.bytes,
        2 * size_of::<CallTargetSnapshot>()
            + size_of::<Vec<CallTargetSnapshot>>()
            + 2 * size_of::<usize>()
    );
    assert!(saved != changed);
    let CallTargetSnapshot::Composed(children) = &saved else {
        panic!("composed target")
    };
    assert!(matches!(
        children[1],
        CallTargetSnapshot::FlowItem(HirItemId(2))
    ));
    drop(changed);
    assert!(matches!(
        children[1],
        CallTargetSnapshot::FlowItem(HirItemId(2))
    ));
}

#[test]
fn consuming_shared_child_tables_copies_headers_not_nested_targets() {
    for shared in [false, true] {
        let children: CallTargetSnapshotChildren = vec![chain(30_000, 0, 1).into_value()].into();
        let retained = shared.then(|| children.clone());
        let (values, cost) = measure(|| children.into_values());
        assert_eq!(cost.count, usize::from(shared));
        assert_eq!(
            cost.bytes,
            usize::from(shared) * size_of::<CallTargetSnapshot>()
        );
        assert_eq!(values.len(), 1);
        drop(values);
        if let Some(retained) = retained {
            assert!(matches!(retained[0], CallTargetSnapshot::Limited { .. }));
        }
    }
}

use super::ContinuationSnapshotLink;
use crate::{orchestration::ContinuationSnapshot, testing::allocation::measure};

#[test]
fn checkpoint_continuation_clone_and_release_do_not_recurse_through_owned_edges() {
    for depth in [32, 1000, 4000, 30_000] {
        let mut snapshot = ContinuationSnapshot::Return;
        for _ in 0..depth {
            snapshot = ContinuationSnapshot::CallBoundary {
                outer: snapshot.into(),
            };
        }
        let (retained, cost) = measure(|| snapshot.clone());
        assert_eq!(
            cost.count, 0,
            "copied continuation tree at depth {depth}: {cost:?}"
        );
        let (_, drop_cost) = measure(|| drop(snapshot));
        assert_eq!(drop_cost.count, 0);
        let mut cursor = &retained;
        for _ in 0..depth {
            let ContinuationSnapshot::CallBoundary { outer } = cursor else {
                panic!("call boundary")
            };
            cursor = outer;
        }
        assert!(matches!(cursor, ContinuationSnapshot::Return));
        let (_, drop_cost) = measure(|| drop(retained));
        assert_eq!(
            drop_cost.count, 0,
            "unary release needs no worklist allocation"
        );
    }
}

fn policy() -> Box<crate::orchestration::ModelExecutionPolicySnapshot> {
    Box::new(crate::orchestration::ModelExecutionPolicySnapshot {
        provider: None,
        provider_capabilities: None,
        model: etas_host::ModelName("test".into()),
        model_locked: false,
        tools: vec![],
        tool_choice: Default::default(),
        policy_ref: None,
        options: Default::default(),
        budget: None,
        response_decode: crate::orchestration::ModelResponseDecodeSnapshot::String,
        max_tool_rounds: 3,
    })
}

#[test]
fn mixed_continuation_edges_release_every_node_after_last_snapshot_owner() {
    use crate::orchestration::{HandlerScopeId, LocalsSnapshot};
    use std::rc::Rc;
    for depth in [1000, 4000, 30_000] {
        let mut node = ContinuationSnapshotLink::new(ContinuationSnapshot::Return);
        let mut weak = Vec::new();
        for i in 0..depth {
            weak.push(Rc::downgrade(node.0.as_ref().unwrap()));
            node = match i % 6 {
                0 => ContinuationSnapshot::CallBoundary { outer: node },
                1 => ContinuationSnapshot::HandlerDispatch { outer: node },
                2 => ContinuationSnapshot::RestoreModelPolicy {
                    previous: policy(),
                    inner: node,
                },
                3 => ContinuationSnapshot::ScopedModelPolicy {
                    policy: policy(),
                    inner: node,
                },
                4 => ContinuationSnapshot::HandleBoundary {
                    scope_id: HandlerScopeId(i as u32),
                    inner: node,
                    handlers: vec![],
                    span: etas_core::Span::empty(etas_core::SourceId(7), etas_core::TextSize::ZERO),
                    frame: LocalsSnapshot {
                        id: i as u64 + 1,
                        locals: Rc::new(vec![]),
                        type_bindings: vec![],
                    },
                },
                _ => {
                    let outer = ContinuationSnapshotLink::new(ContinuationSnapshot::Finish);
                    weak.push(Rc::downgrade(outer.0.as_ref().unwrap()));
                    ContinuationSnapshot::Chain { inner: node, outer }
                }
            }
            .into();
        }
        let retained = node.clone();
        let (_, cost) = measure(|| drop(node));
        assert_eq!(cost.count, 0);
        assert!(weak.iter().all(|node| node.upgrade().is_some()));
        let (_, cost) = measure(|| drop(retained));
        eprintln!("mixed continuation release depth={depth}: {cost:?}");
        assert!(cost.bytes < depth * size_of::<usize>() * 4 + 4096);
        assert!(weak.iter().all(|node| node.upgrade().is_none()));
    }
}

#[test]
fn editing_cloned_continuation_paths_does_not_change_retained_snapshot() {
    let mut original = ContinuationSnapshot::Return;
    for _ in 0..1000 {
        original = ContinuationSnapshot::CallBoundary {
            outer: original.into(),
        };
    }
    let mut changed = original.clone();
    let mut cursor = &mut changed;
    for _ in 0..1000 {
        let ContinuationSnapshot::CallBoundary { outer } = cursor else {
            panic!("call boundary")
        };
        cursor = outer.as_mut();
    }
    *cursor = ContinuationSnapshot::Finish;
    for (snapshot, finish) in [(&original, false), (&changed, true)] {
        let mut cursor = snapshot;
        for _ in 0..1000 {
            let ContinuationSnapshot::CallBoundary { outer } = cursor else {
                panic!("call boundary")
            };
            cursor = outer.as_ref();
        }
        assert_eq!(matches!(cursor, ContinuationSnapshot::Finish), finish);
        assert_eq!(matches!(cursor, ContinuationSnapshot::Return), !finish);
    }
}

#[test]
fn consuming_unique_or_shared_continuation_edges_does_not_copy_descendants() {
    for shared in [false, true] {
        let mut node = ContinuationSnapshotLink::new(ContinuationSnapshot::Return);
        for _ in 0..30_000 {
            node = ContinuationSnapshot::CallBoundary { outer: node }.into();
        }
        let retained = shared.then(|| node.clone());
        let (value, cost) = measure(|| node.into_value());
        assert_eq!(cost.count, 0);
        assert!(matches!(value, ContinuationSnapshot::CallBoundary { .. }));
        drop(value);
        if let Some(retained) = retained {
            assert!(matches!(
                *retained,
                ContinuationSnapshot::CallBoundary { .. }
            ));
        }
    }
}

#[test]
fn shared_continuation_edge_reports_cold_owner_cost() {
    let (old_edge, old_cost) = measure(|| Box::new(ContinuationSnapshot::Return));
    let (edge, cost) = measure(|| ContinuationSnapshotLink::new(ContinuationSnapshot::Return));
    assert_eq!(old_cost.count, 1);
    assert_eq!(cost.count, 1);
    assert_eq!(cost.bytes, old_cost.bytes + 2 * size_of::<usize>());
    drop((old_edge, edge));
}

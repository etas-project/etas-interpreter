use super::*;
use etas_hir::HirItemId;

#[test]
fn shared_value_snapshot_expansion_is_charged_per_occurrence() {
    let graph = |depth| {
        (0..depth).fold(ValueSnapshot::Bool(true), |node, _| {
            ValueSnapshot::Array(vec![node.clone(), node].into())
        })
    };
    let value = graph(30);
    let retained = value.clone();
    let (_, cost) = measure(|| {
        let error =
            encode_with_budget(Node::Value(&value), &mut EncodingBudget::new(128)).unwrap_err();
        assert_eq!(
            error.message(),
            "checkpoint snapshot expansion exceeds node budget"
        );
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "partial graph encoding leaked: {cost:?}"
    );
    assert!(cost.bytes < 128 * 4096, "expanded beyond budget: {cost:?}");
    let (ValueSnapshot::Array(value), ValueSnapshot::Array(retained)) = (&value, &retained) else {
        panic!("array")
    };
    assert_eq!(value.as_ptr(), retained.as_ptr());

    let small = graph(5);
    assert!(encode_with_budget(Node::Value(&small), &mut EncodingBudget::new(62)).is_err());
    let wire = codec::CheckpointDocument::from_value(
        encode_with_budget(Node::Value(&small), &mut EncodingBudget::new(63)).unwrap(),
    );
    let restored = codec::value_from_json(&wire).unwrap();
    assert!(ValueSnapshot::capture(&restored).unwrap() == small);
}

fn shared_target(depth: usize) -> CallTargetSnapshot {
    let mut target = CallTargetSnapshot::FlowItem(HirItemId(7));
    for _ in 0..depth {
        target = CallTargetSnapshot::Composed(vec![target.clone(), target].into());
    }
    target
}

#[test]
fn snapshot_encoding_budget_counts_expanded_occurrences_not_shared_identities() {
    let target = shared_target(12);
    let retained = target.clone();
    let (_, cost) = measure(|| {
        let error = encode_with_budget(Node::CallTarget(&target), &mut EncodingBudget::new(128))
            .unwrap_err();
        assert_eq!(
            error.message(),
            "checkpoint snapshot expansion exceeds node budget"
        );
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "partial JSON leaked: {cost:?}"
    );
    assert!(cost.bytes < 128 * 4096, "expanded past budget: {cost:?}");
    assert!(target == retained);

    let target = shared_target(5);
    // Five binary levels plus leaves unfold to 63 target occurrences.
    assert!(encode_with_budget(Node::CallTarget(&target), &mut EncodingBudget::new(62)).is_err());
    let wire = codec::CheckpointDocument::from_value(
        encode_with_budget(Node::CallTarget(&target), &mut EncodingBudget::new(63)).unwrap(),
    );
    let decoded =
        codec::machine::call_target_snapshot_from_json(&etas_host::StorageLimits::default(), &wire)
            .unwrap();
    assert!(decoded == target);
    eprintln!("snapshot DAG expansion budget=128: {cost:?}");
}

#[test]
fn snapshot_encoding_budget_rejects_fanout_before_allocating_output_slots() {
    let small =
        CallTargetSnapshot::Composed(vec![CallTargetSnapshot::FlowItem(HirItemId(1)); 128].into());
    let large = CallTargetSnapshot::Composed(
        vec![CallTargetSnapshot::FlowItem(HirItemId(1)); 100_000].into(),
    );
    let reject = |target| {
        measure(|| {
            assert!(
                encode_with_budget(Node::CallTarget(target), &mut EncodingBudget::new(32)).is_err()
            );
        })
        .1
    };
    let small = reject(&small);
    let large = reject(&large);
    assert_eq!(small.bytes, large.bytes, "allocated rejected fanout");
    assert_eq!(large.bytes, large.released_bytes);
    assert!(large.bytes < 4096);
}

#[test]
fn snapshot_encoding_budget_rejects_bytes_before_numeric_json_array_allocation() {
    let bytes = ValueSnapshot::Bytes(vec![0; 100_000].into());
    let (_, cost) = measure(|| {
        assert!(encode_with_budget(Node::Value(&bytes), &mut EncodingBudget::new(32)).is_err());
    });
    assert!(cost.bytes < 4096, "materialized rejected bytes: {cost:?}");
    assert_eq!(cost.bytes, cost.released_bytes);
}

#[test]
fn snapshot_encoding_budget_releases_deep_partial_output_iteratively() {
    let mut target = CallTargetSnapshot::FlowItem(HirItemId(7));
    for _ in 0..30_000 {
        target = CallTargetSnapshot::Limited {
            target: target.into(),
            limits: vec![],
        };
    }
    let (_, cost) = measure(|| {
        assert!(
            encode_with_budget(Node::CallTarget(&target), &mut EncodingBudget::new(20_000))
                .is_err()
        );
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "partial deep output leaked: {cost:?}"
    );
    let wire = codec::CheckpointDocument::from_value(
        encode_with_budget(Node::CallTarget(&target), &mut EncodingBudget::default()).unwrap(),
    );
    let mut cursor = &*wire;
    for _ in 0..30_000 {
        assert_eq!(cursor["kind"], "limited");
        cursor = &cursor["target"];
    }
    assert_eq!(cursor["item"], 7);
    eprintln!("partial target expansion depth=20000: {cost:?}");
}

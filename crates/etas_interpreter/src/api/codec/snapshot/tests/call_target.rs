use super::*;
use etas_hir::HirItemId;

#[test]
fn shared_call_target_edges_keep_the_existing_wire_contract_and_decode_checks() {
    let target = CallTargetSnapshot::Specialized {
        target: CallTargetSnapshot::Limited {
            target: CallTargetSnapshot::Composed(
                (vec![
                    CallTargetSnapshot::FlowItem(HirItemId(7)),
                    CallTargetSnapshot::EnumVariant(SymbolId(11)),
                ])
                .into(),
            )
            .into(),
            limits: vec![],
        }
        .into(),
        type_bindings: vec![("T".into(), TypeId(3))],
    };
    let expected = json!({"kind":"specialized", "target": {
        "kind":"limited", "target": {"kind":"composed", "targets":[
            {"kind":"flow", "item":7}, {"kind":"enum_variant", "symbol":11}
        ]}, "limits":[]
    }, "type_bindings":[{"name":"T", "type":3}]});
    let retained = target.clone();
    let encoded = encode(Node::CallTarget(&target));
    assert_eq!(encoded, expected);
    let limits = etas_host::StorageLimits::default();
    let decoded = codec::machine::call_target_snapshot_from_json(&limits, &encoded).unwrap();
    assert!(decoded == retained);
    let runtime = codec::machine::call_target_from_artifact_snapshot(&limits, &encoded).unwrap();
    assert_eq!(codec::machine::call_target_snapshot(&runtime), expected);
    for changed in [
        json!({"kind":"limited", "target":null, "limits":[]}),
        json!({"kind":"composed", "targets":[{"kind":"flow", "item":7}, {"kind":"unknown"}]}),
        json!({"kind":"specialized", "target":{"kind":"flow","item":7}, "type_bindings":[{"name":"T","type":-1}]}),
    ] {
        assert!(codec::machine::call_target_snapshot_from_json(&limits, &changed).is_err());
    }
}

#[test]
fn shared_call_target_snapshot_encoding_does_not_change_after_copy_on_write() {
    let frame = LocalsSnapshot {
        id: 1,
        locals: Rc::new(vec![(SymbolId(1), ValueSnapshot::String("saved".into()))]),
        type_bindings: vec![],
    };
    let mut original = CallTargetSnapshot::Lambda {
        expr: HirExprId(1),
        captured: frame,
    };
    for _ in 0..64 {
        original = CallTargetSnapshot::Limited {
            target: original.into(),
            limits: vec![],
        };
    }
    let original_wire = encode(Node::CallTarget(&original));
    let mut changed = original.clone();
    let mut cursor = &mut changed;
    for _ in 0..64 {
        let CallTargetSnapshot::Limited { target, .. } = cursor else {
            panic!("limited target")
        };
        cursor = target;
    }
    let CallTargetSnapshot::Lambda { captured, .. } = cursor else {
        panic!("lambda target")
    };
    Rc::make_mut(&mut captured.locals)[0].1 = ValueSnapshot::String("changed".into());
    assert!(original != changed);
    assert_eq!(encode(Node::CallTarget(&original)), original_wire);
    let changed_wire = encode(Node::CallTarget(&changed));
    let limits = etas_host::StorageLimits::default();
    assert!(
        codec::machine::call_target_snapshot_from_json(&limits, &changed_wire).unwrap() == changed
    );
    drop(changed);
    assert_eq!(encode(Node::CallTarget(&original)), original_wire);
}

#[test]
fn iterative_call_target_decode_roundtrips_deep_bindings_limits_and_frames_on_wire() {
    use crate::api::codec::CheckpointFileLimits;
    use crate::eval::limit::{RuntimeLimit, RuntimeLimitValue};
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let span = checked.hir.blocks.iter().next().unwrap().1.span;
    let mut target = CallTargetSnapshot::Lambda {
        expr: HirExprId(7),
        captured: LocalsSnapshot {
            id: 19,
            locals: Rc::new(vec![(SymbolId(9), ValueSnapshot::String("saved".into()))]),
            type_bindings: vec![("U".into(), TypeId(3))],
        },
    };
    for i in 0..4000 {
        target = CallTargetSnapshot::Specialized {
            target: CallTargetSnapshot::Limited {
                target: target.into(),
                limits: vec![RuntimeLimit {
                    kind: etas_std::StdLimitKind::Attempts,
                    value: RuntimeLimitValue::Count(i + 1),
                    span,
                }],
            }
            .into(),
            type_bindings: vec![(format!("T{i}"), TypeId(i as u32))],
        };
    }
    let to_bytes = |target: &CallTargetSnapshot| {
        let mut wire =
            json!({"schema":crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA,"checkpoint":null});
        wire["checkpoint"] = encode(Node::CallTarget(target));
        codec::checkpoint_file_to_bytes(wire, CheckpointFileLimits::default()).unwrap()
    };
    let bytes = to_bytes(&target);
    let doc = codec::checkpoint_file_from_bytes(&bytes, CheckpointFileLimits::default()).unwrap();
    let limits = etas_host::StorageLimits::default();
    let decoded =
        codec::machine::call_target_snapshot_from_json(&limits, &doc["checkpoint"]).unwrap();
    assert!(decoded == target);
    assert_eq!(to_bytes(&decoded), bytes);
}

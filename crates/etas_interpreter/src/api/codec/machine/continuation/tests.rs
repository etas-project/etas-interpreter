use super::*;
mod decode;
use crate::{
    api::codec::{self, CheckpointFileLimits},
    control::Continuation,
    orchestration::ValueSnapshot,
    testing::allocation::measure,
    value::InterpValue,
};

#[test]
fn shared_continuation_edges_preserve_wire_and_checked_identity_validation() {
    use crate::eval::machine::snapshot::{RestoreContext, SnapshotValidator};
    use crate::orchestration::{LocalsSnapshot, MachineFrameSnapshot, MachineSnapshot};
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    let wire = json!({"kind":"chain", "inner":{"kind":"return"},
        "outer":{"kind":"call_boundary", "outer":{"kind":"block_value"}}});
    let original = continuation_from_snapshot(&limits, &wire, &checked).unwrap();
    assert_eq!(codec::snapshot::continuation_json(&original), wire);
    let runtime = original
        .clone()
        .restore_with(&mut RestoreContext::default())
        .unwrap();
    let recaptured = ContinuationSnapshot::capture(&runtime).unwrap();
    assert_eq!(codec::snapshot::continuation_json(&recaptured), wire);
    let validator = SnapshotValidator::new(
        &checked,
        &plan.slots,
        &plan.dispatch,
        &plan.closures,
        &limits,
    );
    let machine = |continuation| MachineSnapshot {
        frames: vec![MachineFrameSnapshot::Continuation { continuation }],
    };
    validator
        .validate_machine(&machine(original.clone()))
        .unwrap();
    let mut changed = original.clone();
    let ContinuationSnapshot::Chain { outer, .. } = &mut changed else {
        panic!("chain")
    };
    let ContinuationSnapshot::CallBoundary { outer } = outer.as_mut() else {
        panic!("call boundary")
    };
    *outer.as_mut() = ContinuationSnapshot::ContinueBlock {
        block: HirBlockId(u32::MAX),
        next_stmt_index: 0,
        frame: LocalsSnapshot {
            id: 1,
            locals: Default::default(),
            type_bindings: vec![],
        },
    };
    assert!(
        validator
            .validate_machine(&machine(changed))
            .unwrap_err()
            .contains("block")
    );
    assert_eq!(codec::snapshot::continuation_json(&original), wire);
    validator.validate_machine(&machine(original)).unwrap();
    let mut invalid = wire;
    invalid["outer"]["outer"]["kind"] = json!("invalid-continuation");
    assert!(
        continuation_from_snapshot(&limits, &invalid, &checked)
            .unwrap_err()
            .contains("unknown machine continuation")
    );
}

#[test]
fn continuation_decode_builds_snapshot_locals_without_a_temporary_runtime_frame() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let limits = etas_host::StorageLimits::default();
    for count in [1000, 2000, 4000] {
        let wire = json!({
            "kind":"continue_block", "block":0, "next_stmt_index":0,
            "frame": { "id":73, "type_bindings":[], "locals":
                (0..count).map(|symbol| json!({"symbol":symbol,
                    "value":{"kind":"string", "value":"x".repeat(1024)}})).collect::<Vec<_>>()
            }
        });
        let (actual, direct) =
            measure(|| continuation_from_snapshot(&limits, &wire, &checked).unwrap());
        // The former decode boundary built runtime locals and a live Frame,
        // only to immediately capture them again. This is a test reference.
        let (expected, staged) = measure(|| {
            let locals = wire["frame"]["locals"]
                .as_array()
                .unwrap()
                .iter()
                .map(|local| {
                    (
                        SymbolId(local["symbol"].as_u64().unwrap() as u32),
                        codec::value_from_json_with_limits(&limits, &local["value"]).unwrap(),
                    )
                })
                .collect();
            let mut frame = Frame::from_snapshot(locals).unwrap();
            frame.set_snapshot_id(73).unwrap();
            ContinuationSnapshot::capture(&Continuation::ContinueBlock {
                block: HirBlockId(0),
                next_stmt_index: 0,
                frame,
            })
            .unwrap()
        });
        assert_eq!(
            codec::snapshot::continuation_json(&actual),
            codec::snapshot::continuation_json(&expected)
        );
        eprintln!("decode locals n={count}: direct={direct:?}, staged={staged:?}");
        assert!(direct.count < staged.count);
        assert!(direct.bytes + count * std::mem::size_of::<InterpValue>() < staged.bytes);
        let ContinuationSnapshot::ContinueBlock { frame, .. } = actual else {
            panic!("block")
        };
        assert_eq!(frame.id, 73);
        assert_eq!(frame.locals.len(), count);
    }
}

#[test]
fn direct_continuation_decode_preserves_deep_payloads_and_rejects_late_invalid_nodes() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let limits = etas_host::StorageLimits::default();
    for depth in [1000, 4000, 30000] {
        let mut value = ValueSnapshot::String("leaf".into());
        for _ in 0..depth {
            value = ValueSnapshot::OptionSome(crate::orchestration::SnapshotBox::new(value));
        }
        let original = ContinuationSnapshot::ListConsTail {
            head: value,
            span: checked.hir.blocks.iter().next().unwrap().1.span,
        };
        let wire = codec::snapshot::continuation_json(&original);
        // The bounded file document also owns and releases deep JSON iteratively.
        let mut artifact =
            json!({"schema":crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA,"checkpoint":null});
        artifact["checkpoint"] = wire;
        let file =
            codec::checkpoint_file_to_bytes(artifact, CheckpointFileLimits::default()).unwrap();
        let document =
            codec::checkpoint_file_from_bytes(&file, CheckpointFileLimits::default()).unwrap();
        let actual =
            continuation_from_snapshot(&limits, &document["checkpoint"], &checked).unwrap();
        let (
            ContinuationSnapshot::ListConsTail { head: actual, .. },
            ContinuationSnapshot::ListConsTail { head: expected, .. },
        ) = (actual, original)
        else {
            panic!("list tail")
        };
        assert_eq!(actual, expected);
        let mut invalid = json!({"kind":"aggregate_element", "expr":0, "next_index":0,
            "frame":{"id":1,"locals":[],"type_bindings":[]}, "values":[]});
        // A completed deep child must be released safely if the next node fails.
        let mut expected_wire =
            codec::snapshot::continuation_json(&ContinuationSnapshot::ListConsTail {
                head: expected,
                span: checked.hir.blocks.iter().next().unwrap().1.span,
            });
        invalid["values"]
            .as_array_mut()
            .unwrap()
            .push(expected_wire["head"].take());
        invalid["values"]
            .as_array_mut()
            .unwrap()
            .push(json!({"kind":"host_handle"}));
        let mut artifact =
            json!({"schema":crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA,"checkpoint":null});
        artifact["checkpoint"] = invalid;
        let file =
            codec::checkpoint_file_to_bytes(artifact, CheckpointFileLimits::default()).unwrap();
        let invalid =
            codec::checkpoint_file_from_bytes(&file, CheckpointFileLimits::default()).unwrap();
        let error =
            continuation_from_snapshot(&limits, &invalid["checkpoint"], &checked).unwrap_err();
        assert!(error.contains("live host capability"), "{error}");
    }
}

#[test]
fn direct_frame_decode_rejects_zero_identity_and_duplicate_bindings() {
    let limits = etas_host::StorageLimits::default();
    for wire in [
        json!({"id":0,"locals":[],"type_bindings":[]}),
        json!({"id":1,"locals":[
            {"symbol":7,"value":{"kind":"unit"}},
            {"symbol":7,"value":{"kind":"unit"}}],"type_bindings":[]}),
        json!({"id":1,"locals":[],"type_bindings":[
            {"name":"T","type":1},{"name":"T","type":2}]}),
    ] {
        assert!(locals_from_snapshot(&limits, &wire).is_err());
    }
}

#[test]
fn callable_decode_preserves_artifact_frame_identity_in_both_representations() {
    let limits = etas_host::StorageLimits::default();
    let wire = json!({"kind":"lambda","expr":3,"captured":{
        "id":7341, "locals":[{"symbol":7,"value":{"kind":"string","value":"kept"}}],
        "type_bindings":[{"name":"T","type":4}]
    }});
    let snapshot =
        super::super::call_target::call_target_snapshot_from_json(&limits, &wire).unwrap();
    let runtime =
        super::super::call_target::call_target_from_artifact_snapshot(&limits, &wire).unwrap();
    let crate::orchestration::CallTargetSnapshot::Lambda {
        captured: snapshot, ..
    } = snapshot
    else {
        panic!("lambda snapshot")
    };
    let crate::control::CallTarget::Lambda {
        captured: runtime, ..
    } = runtime
    else {
        panic!("runtime lambda")
    };
    assert_eq!(snapshot.id, 7341);
    assert_eq!(runtime.snapshot_id(), snapshot.id);
    assert_eq!(runtime.sorted_type_bindings(), snapshot.type_bindings);
    assert_eq!(
        runtime.get(SymbolId(7)),
        Some(snapshot.locals[0].1.clone().restore().unwrap())
    );
}

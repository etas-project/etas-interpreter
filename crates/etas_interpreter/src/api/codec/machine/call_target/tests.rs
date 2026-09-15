use super::*;
use crate::{
    api::codec::{self, CheckpointDocument, CheckpointFileLimits},
    orchestration::CallTargetSnapshot,
    testing::allocation::measure,
};

fn document(root: Value) -> CheckpointDocument {
    let mut artifact =
        json!({"schema":crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA,"checkpoint":null});
    artifact["checkpoint"] = root;
    let bytes = codec::checkpoint_file_to_bytes(artifact, CheckpointFileLimits::default()).unwrap();
    codec::checkpoint_file_from_bytes(&bytes, CheckpointFileLimits::default()).unwrap()
}

fn wire(depth: usize, mode: usize) -> Value {
    let mut value = json!({"kind":"flow","item":1});
    for _ in 0..depth {
        let mut parent = match mode {
            0 => json!({"kind":"limited","limits":[],"target":null}),
            1 => json!({"kind":"specialized","type_bindings":[],"target":null}),
            2 => json!({"kind":"composed","targets":[null,{"kind":"flow","item":2}]}),
            _ => json!({"kind":"composed","targets":[{"kind":"flow","item":2},null]}),
        };
        match mode {
            0 | 1 => parent["target"] = value,
            2 => parent["targets"][0] = value,
            _ => parent["targets"][1] = value,
        }
        value = parent;
    }
    value
}

// No custom cleanup: both successful and failed decodes use production Drop.
struct RuntimeTree(Option<CallTarget>);

fn check_snapshot(mut target: &CallTargetSnapshot, depth: usize, mode: usize) {
    for _ in 0..depth {
        target = match (mode, target) {
            (0, CallTargetSnapshot::Limited { target, limits }) => {
                assert!(limits.is_empty());
                target
            }
            (
                1,
                CallTargetSnapshot::Specialized {
                    target,
                    type_bindings,
                },
            ) => {
                assert!(type_bindings.is_empty());
                target
            }
            (2 | 3, CallTargetSnapshot::Composed(children)) => {
                assert_eq!(children.len(), 2);
                let index = usize::from(mode == 3);
                assert!(matches!(
                    children[1 - index],
                    CallTargetSnapshot::FlowItem(etas_hir::HirItemId(2))
                ));
                &children[index]
            }
            _ => panic!("changed snapshot topology"),
        };
    }
    assert!(matches!(
        target,
        CallTargetSnapshot::FlowItem(etas_hir::HirItemId(1))
    ));
}

fn check_runtime(mut target: &CallTarget, depth: usize, mode: usize) {
    for _ in 0..depth {
        target = match (mode, target) {
            (0, CallTarget::Limited { target, limits }) => {
                assert!(limits.is_empty());
                target
            }
            (
                1,
                CallTarget::Specialized {
                    target,
                    type_bindings,
                },
            ) => {
                assert!(type_bindings.is_empty());
                target
            }
            (2 | 3, CallTarget::Composed(children)) => {
                assert_eq!(children.len(), 2);
                let index = usize::from(mode == 3);
                assert!(matches!(
                    children[1 - index],
                    CallTarget::FlowItem(etas_hir::HirItemId(2))
                ));
                &children[index]
            }
            _ => panic!("changed runtime topology"),
        };
    }
    assert!(matches!(
        target,
        CallTarget::FlowItem(etas_hir::HirItemId(1))
    ));
}

#[test]
fn deep_call_target_json_decode_uses_a_worklist_for_both_representations() {
    const WORKER: &str = "ETAS_TEST_CALL_TARGET_DECODE_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let current = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "decoder subprocess failed: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    let limits = etas_host::StorageLimits::default();
    for depth in [1000, 4000, 30_000] {
        for mode in 0..4 {
            let doc = document(wire(depth, mode));
            let (saved, cost) =
                measure(|| call_target_snapshot_from_json(&limits, &doc["checkpoint"]).unwrap());
            eprintln!("call target snapshot decode depth={depth} mode={mode}: {cost:?}");
            assert!(
                cost.count <= depth * if mode < 2 { 1 } else { 2 } + 32,
                "intermediate graph: {cost:?}"
            );
            check_snapshot(&saved, depth, mode);
            let (runtime, cost) = measure(|| {
                RuntimeTree(Some(
                    call_target_from_artifact_snapshot(&limits, &doc["checkpoint"]).unwrap(),
                ))
            });
            eprintln!("call target runtime decode depth={depth} mode={mode}: {cost:?}");
            assert!(
                cost.count <= depth * if mode < 2 { 1 } else { 2 } + 32,
                "intermediate graph: {cost:?}"
            );
            check_runtime(runtime.0.as_ref().unwrap(), depth, mode);
        }
    }
}

#[test]
fn call_target_decoder_preserves_child_then_metadata_error_order() {
    let limits = etas_host::StorageLimits::default();
    let cases = [
        (
            json!({"kind":"specialized","target":{"kind":"bad-child"},"type_bindings":false}),
            "unknown machine call target `bad-child`",
        ),
        (
            json!({"kind":"specialized","target":{"kind":"flow","item":1},"type_bindings":[{"name":"T","type":-1}]}),
            "`type`",
        ),
        (
            json!({"kind":"limited","target":{"kind":"bad-child"},"limits":false}),
            "unknown machine call target `bad-child`",
        ),
        (
            json!({"kind":"composed","targets":[{"kind":"first"},{"kind":"second"}]}),
            "unknown machine call target `first`",
        ),
        (
            json!({"kind":"composed","targets":false}),
            "call targets must be an array",
        ),
        (
            json!({"kind":"lambda","expr":"bad","captured":{}}),
            "`expr`",
        ),
    ];
    for (value, expected) in cases {
        let error = call_target_snapshot_from_json(&limits, &value).unwrap_err();
        assert!(error.contains(expected), "{error}");
        let error = call_target_from_artifact_snapshot(&limits, &value).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn call_target_decode_late_failures_release_partial_deep_runtime_and_snapshot_targets() {
    let limits = etas_host::StorageLimits::default();
    for mode in 0..4 {
        for case in 0..3 {
            let mut parent = match case {
                0 => json!({"kind":"specialized","target":null,"type_bindings":false}),
                1 => json!({"kind":"limited","target":null,"limits":false}),
                _ => json!({"kind":"composed","targets":[null,{"kind":"late-invalid"}]}),
            };
            if case == 2 {
                parent["targets"][0] = wire(30_000, mode);
            } else {
                parent["target"] = wire(30_000, mode);
            }
            let doc = document(parent);
            for _ in 0..2 {
                let (_, cost) = measure(|| {
                    assert!(call_target_snapshot_from_json(&limits, &doc["checkpoint"]).is_err());
                });
                assert_eq!(
                    cost.bytes, cost.released_bytes,
                    "snapshot mode={mode} case={case}: {cost:?}"
                );
                let (_, cost) = measure(|| {
                    match call_target_from_artifact_snapshot(&limits, &doc["checkpoint"]) {
                        Err(_) => {}
                        Ok(value) => {
                            drop(RuntimeTree(Some(value)));
                            panic!("accepted invalid target");
                        }
                    }
                });
                assert_eq!(
                    cost.bytes, cost.released_bytes,
                    "runtime mode={mode} case={case}: {cost:?}"
                );
            }
        }
    }
}

#[test]
fn wide_call_target_decode_only_allocates_the_output_table_and_frontier() {
    let limits = etas_host::StorageLimits::default();
    for width in [1000, 4000, 30_000] {
        let wire = json!({"kind":"composed", "targets":(0..width)
            .map(|i| json!({"kind":"flow","item":i})).collect::<Vec<_>>()});
        let (saved, cost) = measure(|| call_target_snapshot_from_json(&limits, &wire).unwrap());
        eprintln!("wide snapshot decode width={width}: {cost:?}");
        assert_eq!(
            cost.count, 3,
            "output table, shared owner, traversal frontier"
        );
        assert!(cost.bytes <= width * size_of::<CallTargetSnapshot>() + 512);
        let CallTargetSnapshot::Composed(targets) = saved else {
            panic!("composed")
        };
        for (i, target) in targets.iter().enumerate() {
            assert!(
                matches!(target, CallTargetSnapshot::FlowItem(etas_hir::HirItemId(id)) if *id as usize == i)
            );
        }
        let (runtime, cost) =
            measure(|| call_target_from_artifact_snapshot(&limits, &wire).unwrap());
        eprintln!("wide runtime decode width={width}: {cost:?}");
        assert_eq!(
            cost.count, 3,
            "output table, shared owner and traversal frontier"
        );
        assert!(cost.bytes <= width * size_of::<CallTarget>() + 512);
        let CallTarget::Composed(targets) = runtime else {
            panic!("composed")
        };
        for (i, target) in targets.iter().enumerate() {
            assert!(
                matches!(target, CallTarget::FlowItem(etas_hir::HirItemId(id)) if *id as usize == i)
            );
        }
    }
    let empty = json!({"kind":"composed","targets":[]});
    assert!(
        matches!(call_target_snapshot_from_json(&limits, &empty).unwrap(), CallTargetSnapshot::Composed(values) if values.is_empty())
    );
    assert!(
        matches!(call_target_from_artifact_snapshot(&limits, &empty).unwrap(), CallTarget::Composed(values) if values.is_empty())
    );
}

#[test]
fn decoded_deep_targets_still_require_checked_machine_identity() {
    use crate::{
        eval::machine::snapshot::SnapshotValidator,
        orchestration::{ContinuationSnapshot, MachineFrameSnapshot, MachineSnapshot},
    };
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    let validator = SnapshotValidator::new(
        &checked,
        &plan.slots,
        &plan.dispatch,
        &plan.closures,
        &limits,
    );
    let span = checked.hir.blocks.iter().next().unwrap().1.span;
    let entry = checked.entry.unwrap();
    for (leaf, valid) in [
        (json!({"kind":"flow","item":entry.0}), true),
        (json!({"kind":"flow","item":u32::MAX}), false),
        (
            json!({"kind":"pure_intrinsic","intrinsic":u32::MAX,"parameter_types":[],"result_type":0}),
            false,
        ),
        (json!({"kind":"enum_variant","symbol":u32::MAX}), false),
    ] {
        let mut tree = leaf;
        for _ in 0..30_000 {
            let mut parent = json!({"kind":"limited","limits":[],"target":null});
            parent["target"] = tree;
            tree = parent;
        }
        let doc = document(tree);
        let target = call_target_snapshot_from_json(&limits, &doc["checkpoint"]).unwrap();
        let machine = MachineSnapshot {
            frames: vec![MachineFrameSnapshot::Continuation {
                continuation: ContinuationSnapshot::ComposedCall {
                    remaining: vec![target],
                    span,
                },
            }],
        };
        let validation = validator.validate_machine(&machine);
        assert_eq!(validation.is_ok(), valid, "{validation:?}");
    }
}

use super::*;

fn file(root: Value) -> Vec<u8> {
    let mut artifact =
        json!({"schema":crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA,"checkpoint":null});
    artifact["checkpoint"] = root;
    codec::checkpoint_file_to_bytes(artifact, CheckpointFileLimits::default()).unwrap()
}

fn document(root: Value) -> codec::CheckpointDocument {
    codec::checkpoint_file_from_bytes(&file(root), CheckpointFileLimits::default()).unwrap()
}

#[test]
fn deep_continuation_file_decode_avoids_recursive_descent_and_preserves_wire() {
    const WORKER: &str = "ETAS_TEST_CONTINUATION_DECODE_WORKER";
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
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let limits = etas_host::StorageLimits::default();
    for depth in [1000, 4000, 30_000] {
        for mode in 0..3 {
            let mut original = ContinuationSnapshot::Return;
            for _ in 0..depth {
                original = match mode {
                    0 => ContinuationSnapshot::CallBoundary {
                        outer: original.into(),
                    },
                    1 => ContinuationSnapshot::Chain {
                        inner: original.into(),
                        outer: ContinuationSnapshot::Finish.into(),
                    },
                    _ => ContinuationSnapshot::Chain {
                        inner: ContinuationSnapshot::Resume.into(),
                        outer: original.into(),
                    },
                };
            }
            let encoded = file(codec::snapshot::continuation_json(&original));
            let document =
                codec::checkpoint_file_from_bytes(&encoded, CheckpointFileLimits::default())
                    .unwrap();
            let (decoded, cost) = measure(|| {
                continuation_from_snapshot(&limits, &document["checkpoint"], &checked).unwrap()
            });
            eprintln!("continuation decode depth={depth} mode={mode}: {cost:?}");
            let edges = depth * if mode == 0 { 1 } else { 2 };
            assert!(
                cost.count <= edges + 32,
                "copied input/snapshot graph: {cost:?}"
            );
            assert!(
                cost.bytes
                    <= edges * (size_of::<ContinuationSnapshot>() + 2 * size_of::<usize>())
                        + depth * 128
                        + 4096
            );
            let reencoded = file(codec::snapshot::continuation_json(&decoded));
            assert_eq!(reencoded, encoded);
        }
    }
}

fn deep_wire(depth: usize) -> Value {
    let mut root = ContinuationSnapshot::Return;
    for _ in 0..depth {
        root = ContinuationSnapshot::CallBoundary { outer: root.into() };
    }
    codec::snapshot::continuation_json(&root)
}

#[test]
fn continuation_decoder_preserves_prefix_and_child_error_order() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let limits = etas_host::StorageLimits::default();
    let cases = [
        (
            json!({"kind":"chain","inner":{"kind":"invalid-inner"}}),
            "unknown machine continuation `invalid-inner`",
        ),
        (
            json!({"kind":"chain","outer":{"kind":"invalid-outer"}}),
            "`inner`",
        ),
        (
            json!({"kind":"handle_boundary","scope_id":"invalid","inner":{"kind":"invalid-inner"}}),
            "`scope_id`",
        ),
        (
            json!({"kind":"handle_boundary","scope_id":1,"inner":{"kind":"invalid-inner"}}),
            "unknown machine continuation `invalid-inner`",
        ),
        (
            json!({"kind":"restore_model_policy","previous":{},"inner":{"kind":"invalid-inner"}}),
            "`provider_capabilities`",
        ),
        (
            json!({"kind":"scoped_model_policy","policy":{},"inner":{"kind":"invalid-inner"}}),
            "`provider_capabilities`",
        ),
    ];
    for (wire, expected) in cases {
        let error = continuation_from_snapshot(&limits, &wire, &checked).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn continuation_decoder_rejects_late_errors_without_leaking_partial_deep_tree() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let limits = etas_host::StorageLimits::default();
    for case in 0..6 {
        let mut wire = if case < 4 {
            json!({"kind":"chain","inner":null})
        } else {
            json!({"kind":"handle_boundary","scope_id":1,"inner":null,"handlers":[]})
        };
        wire["inner"] = deep_wire(30_000);
        let expected = match case {
            0 => "`outer`",
            1 => {
                wire["outer"] = json!({"kind":"invalid-outer"});
                "unknown machine continuation"
            }
            2 => {
                wire["outer"] = Value::Null;
                "`kind`"
            }
            3 => {
                wire["outer"] = json!({"kind":"pipeline_target","input":{"kind":"host_handle"}});
                "live host capability"
            }
            4 => {
                wire["handlers"] = json!(false);
                "`handlers` must be an array"
            }
            _ => {
                wire["span"] = crate::api::codec::machine::frame::span_snapshot(
                    checked.hir.blocks.iter().next().unwrap().1.span,
                );
                wire["frame"] = json!({"id":0,"locals":[],"type_bindings":[]});
                "nonzero"
            }
        };
        let doc = document(wire);
        for _ in 0..2 {
            let (_, cost) = measure(|| {
                let error = match continuation_from_snapshot(&limits, &doc["checkpoint"], &checked)
                {
                    Err(error) => error,
                    Ok(_) => panic!("accepted malformed continuation"),
                };
                assert!(error.contains(expected), "case={case}: {error}");
            });
            assert_eq!(
                cost.bytes, cost.released_bytes,
                "case={case}: retained partial decode allocation: {cost:?}"
            );
        }
    }
}

fn policy(i: usize) -> Box<crate::orchestration::ModelExecutionPolicySnapshot> {
    Box::new(crate::orchestration::ModelExecutionPolicySnapshot {
        provider: None,
        provider_capabilities: None,
        model: etas_host::ModelName(format!("model-{i}")),
        model_locked: i.is_multiple_of(2),
        tools: vec![],
        tool_choice: Default::default(),
        policy_ref: None,
        options: Default::default(),
        budget: None,
        response_decode: crate::orchestration::ModelResponseDecodeSnapshot::String,
        max_tool_rounds: i + 1,
    })
}

#[test]
fn mixed_continuation_decode_preserves_all_wrapper_metadata() {
    use crate::orchestration::{HandlerScopeId, LocalsSnapshot};
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let limits = etas_host::StorageLimits::default();
    let span = checked.hir.blocks.iter().next().unwrap().1.span;
    let mut root = ContinuationSnapshot::Finish;
    for i in 0..4000 {
        root = match i % 6 {
            0 => ContinuationSnapshot::CallBoundary { outer: root.into() },
            1 => ContinuationSnapshot::HandlerDispatch { outer: root.into() },
            2 => ContinuationSnapshot::HandleBoundary {
                scope_id: HandlerScopeId(i as u32),
                inner: root.into(),
                handlers: vec![],
                span,
                frame: LocalsSnapshot {
                    id: i as u64 + 1,
                    locals: Default::default(),
                    type_bindings: vec![],
                },
            },
            3 => ContinuationSnapshot::RestoreModelPolicy {
                previous: policy(i),
                inner: root.into(),
            },
            4 => ContinuationSnapshot::ScopedModelPolicy {
                policy: policy(i),
                inner: root.into(),
            },
            _ => ContinuationSnapshot::Chain {
                inner: root.into(),
                outer: ContinuationSnapshot::BlockValue.into(),
            },
        };
    }
    let original = file(codec::snapshot::continuation_json(&root));
    let doc =
        codec::checkpoint_file_from_bytes(&original, CheckpointFileLimits::default()).unwrap();
    let decoded = continuation_from_snapshot(&limits, &doc["checkpoint"], &checked).unwrap();
    assert_eq!(file(codec::snapshot::continuation_json(&decoded)), original);
}

#[test]
fn deep_checkpoint_file_decodes_through_checked_boundary_and_enforces_file_limits() {
    use crate::orchestration::*;
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let entry = checked.entry.unwrap();
    for invalid_leaf in [false, true] {
        let mut root = if invalid_leaf {
            ContinuationSnapshot::ContinueBlock {
                block: HirBlockId(u32::MAX),
                next_stmt_index: 0,
                frame: LocalsSnapshot {
                    id: 1,
                    locals: Default::default(),
                    type_bindings: vec![],
                },
            }
        } else {
            ContinuationSnapshot::Return
        };
        for _ in 0..30_000 {
            root = ContinuationSnapshot::CallBoundary { outer: root.into() };
        }
        let saved = InterpreterCheckpoint {
            id: CheckpointId(1),
            label: None,
            compilation: CheckpointCompilationIdentity::for_project(&checked, entry).unwrap(),
            entry_item: entry,
            args: vec![],
            machine: MachineSnapshot {
                frames: vec![MachineFrameSnapshot::Continuation { continuation: root }],
            },
            handlers: Default::default(),
            retry_state: Default::default(),
            trace: Default::default(),
            execution_progress: ExecutionProgressSnapshot {
                consumed_steps: 0,
                original_limits: Default::default(),
            },
            host_state: CheckpointHostState {
                trace: etas_host::TraceContext::root(etas_host::TraceId(1)),
                budget: CheckpointBudgetSnapshot::capture(&etas_host::ExecutionBudget::default())
                    .unwrap(),
            },
            storage: StorageSnapshot {
                identity: etas_host::StorageOperationKey::new(std::time::Duration::from_secs(3600))
                    .unwrap(),
                operations: Default::default(),
                writes: vec![],
            },
            current_session: None,
            resource_versions: Default::default(),
            completed_host_boundaries: Default::default(),
        };
        let artifact =
            codec::checkpoint_artifact_json(&[std::path::PathBuf::from("main.es")], "main", &saved)
                .unwrap();
        let bytes =
            codec::checkpoint_file_to_bytes(artifact, CheckpointFileLimits::default()).unwrap();
        for (budget, expected) in [
            (
                CheckpointFileLimits {
                    max_bytes: bytes.len() - 1,
                    ..Default::default()
                },
                "byte budget",
            ),
            (
                CheckpointFileLimits {
                    max_nodes: 100,
                    ..Default::default()
                },
                "node budget",
            ),
        ] {
            let error = match codec::checkpoint_file_from_bytes(&bytes, budget) {
                Err(error) => error,
                Ok(_) => panic!("ignored checkpoint file budget"),
            };
            assert!(error.message().contains(expected), "{error}");
        }
        let doc =
            codec::checkpoint_file_from_bytes(&bytes, CheckpointFileLimits::default()).unwrap();
        let result = codec::checkpoint_from_json_with_limits(
            &etas_host::StorageLimits::default(),
            &doc,
            &checked,
        );
        if invalid_leaf {
            match result {
                Err(error) => assert!(error.message().contains("block")),
                Ok(_) => panic!("accepted invalid checked checkpoint leaf"),
            }
        } else {
            let decoded = result.unwrap();
            let artifact = codec::checkpoint_artifact_json(
                &[std::path::PathBuf::from("main.es")],
                "main",
                &decoded,
            )
            .unwrap();
            assert_eq!(
                codec::checkpoint_file_to_bytes(artifact, CheckpointFileLimits::default()).unwrap(),
                bytes
            );
        }
    }
}

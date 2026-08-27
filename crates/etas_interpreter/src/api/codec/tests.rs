use super::*;
use crate::orchestration::{
    ActiveHandlerArmRecord, HandlerSnapshot, HostBoundaryLedger, LocalsSnapshot,
    ResourceVersionSnapshot,
};

fn test_span(start: u32, end: u32) -> Span {
    Span::new(SourceId(7), TextRange::new(TextSize(start), TextSize(end)))
}

fn checked_project() -> etas_frontend::CheckedProject {
    crate::testing::project::checked_project("module app.main; flow main() -> unit { return; }")
}

fn minimal_checkpoint_artifact(checked: &etas_frontend::CheckedProject) -> Value {
    let entry = checked.entry.expect("test entry");
    let checkpoint = InterpreterCheckpoint {
        id: CheckpointId(0),
        label: None,
        compilation: CheckpointCompilationIdentity::for_project(checked, entry)
            .expect("test compilation identity"),
        entry_item: entry,
        args: Vec::new(),
        machine: MachineSnapshot::default(),
        handlers: HandlerSnapshot::default(),
        retry_state: RetrySnapshot::default(),
        trace: TraceSnapshot::default(),
        execution_progress: ExecutionProgressSnapshot {
            consumed_steps: 0,
            original_limits: crate::api::ExecutionLimits::default(),
        },
        host_context: HostExecutionContext::default(),
        current_session: None,
        resource_versions: ResourceVersionSnapshot::default(),
        completed_host_boundaries: HostBoundaryLedger::default(),
    };
    checkpoint_artifact_json(&[PathBuf::from("main.es")], "main", &checkpoint)
        .expect("minimal checkpoint artifact should encode")
}

#[test]
fn approval_trace_json_redacts_sensitive_request_fields() {
    let request = etas_host::ApprovalRequest {
        id: etas_host::HostRequestId(41),
        reason: "do not persist this approval reason".to_owned(),
        requested_grants: vec![HostActionGrant::allow("Console", "stdout_write")],
        trace: TraceContext::root(TraceId(42)),
    };
    let payload = etas_host::HostTraceRequest::trace_payload(&request);
    let metadata = etas_host::HostTraceMetadata::from_payload(
        &payload,
        &etas_host::HostTraceDigestKey::from_bytes([7; 32]),
    )
    .expect("approval trace metadata should materialize");
    let event = WorkflowEvent::HostTrace(etas_host::TraceEvent::ApprovalRequested {
        id: request.id,
        metadata,
        trace: request.trace,
    });

    let encoded = event_json(&event);
    let encoded_text = encoded.to_string();
    assert!(!encoded_text.contains("do not persist this approval reason"));
    assert_eq!(encoded["payload"][0]["name"], "reason");
    assert_eq!(encoded["payload"][0]["sensitivity"], "sensitive");
    assert!(encoded["payload"][0]["value"].is_null());
    assert!(
        encoded["payload_digest"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
}

#[test]
fn value_codec_round_trips_every_numeric_width_and_nominal_identity() {
    use crate::value::NumericValue;

    let values = vec![
        NumericValue::I8(i8::MIN),
        NumericValue::I16(i16::MIN),
        NumericValue::I32(i32::MIN),
        NumericValue::I64(i64::MIN),
        NumericValue::I128(i128::MIN),
        NumericValue::ISize(i64::MIN),
        NumericValue::U8(u8::MAX),
        NumericValue::U16(u16::MAX),
        NumericValue::U32(u32::MAX),
        NumericValue::U64(u64::MAX),
        NumericValue::U128(u128::MAX),
        NumericValue::USize(u64::MAX),
        NumericValue::F32((-0.0_f32).to_bits()),
        NumericValue::F64(f64::NAN.to_bits()),
    ]
    .into_iter()
    .map(InterpValue::Number)
    .collect::<Vec<_>>();
    let expected = InterpValue::Nominal {
        ty: etas_types::TypeId(27),
        value: Box::new(InterpValue::Tuple(values)),
    };

    let encoded = value_json(&expected);
    let restored = value_from_json(&encoded).expect("numeric value codec must be lossless");
    assert_eq!(restored, expected);
}

#[test]
fn value_codec_redacts_and_rejects_sealed_host_capabilities() {
    let value = InterpValue::HostHandle(crate::value::HostHandleValue::tcp_stream(
        etas_types::TypeId(42),
        etas_host::TcpStreamRef::issued(
            etas_host::StreamHandleRef::issued("private-stream-id", 0),
            etas_host::ByteStreamOrigin::Tcp {
                host: "example.test".to_owned(),
                port: 443,
            },
        ),
    ));

    let encoded = value_json(&value);
    assert_eq!(encoded["kind"], "host_handle");
    assert_eq!(encoded["handle_kind"], "tcp_stream");
    assert!(!encoded.to_string().contains("private-stream-id"));

    let error = value_from_json(&encoded)
        .expect_err("serialized data must not mint a live host capability");
    assert!(error.message().contains("cannot be restored"), "{error:?}");
}

#[test]
fn checkpoint_codec_round_trips_handlers_and_resource_versions() {
    let checked = crate::testing::project::checked_project(
        r#"
module app.main;

effect Gate {
  action request(value: string) -> string;
}

flow main() -> unit {
  let value = perform Gate.request("value") with {
    Gate.request(value) => resume value;
  };
  return;
}
"#,
    );
    let (_, hir_arm) = checked
        .hir
        .handler_arms
        .iter()
        .next()
        .expect("test project should contain a handler arm");
    let action_symbol = match hir_arm.action.action_symbol {
        etas_hir::ResolveResult::Resolved(symbol) => Some(symbol),
        ref other => panic!("test handler action should resolve, got {other:?}"),
    };
    let scope_id = crate::orchestration::HandlerScopeId(0);
    let handler_arm = ActiveHandlerArmRecord {
        effect_segments: vec!["Console".to_owned()],
        action: "read_line".to_owned(),
        action_symbol,
        type_args: vec![],
        effect_type_args: vec![],
        patterns: hir_arm.patterns.clone(),
        body: hir_arm.body,
        scope: hir_arm.scope,
        span: hir_arm.span,
    };
    let checkpoint = InterpreterCheckpoint {
        id: CheckpointId(3),
        label: Some("inside handler".to_owned()),
        compilation: CheckpointCompilationIdentity::for_project(
            &checked,
            checked.entry.expect("test entry item"),
        )
        .expect("test project identity"),
        entry_item: checked.entry.expect("test entry item"),
        args: vec![InterpValue::i32(42)],
        machine: MachineSnapshot {
            frames: vec![MachineFrameSnapshot::Handler {
                continuation: ContinuationSnapshot::HandleBoundary {
                    scope_id,
                    inner: Box::new(ContinuationSnapshot::BlockValue),
                    handlers: vec![handler_arm.clone()],
                    span: test_span(1, 10),
                    frame: LocalsSnapshot { locals: Vec::new() },
                },
            }],
        },
        handlers: HandlerSnapshot {
            handlers: vec![ActiveHandlerRecord {
                id: scope_id,
                handled_actions: vec!["Console.read_line".to_owned()],
                handlers: vec![handler_arm],
                span: test_span(1, 10),
            }],
        },
        retry_state: RetrySnapshot {
            attempts: vec![RetryAttemptRecord {
                id: RetryAttemptId(41),
                ordinal: 1,
            }],
        },
        trace: TraceSnapshot {
            events_recorded: 4,
            next_message: 2,
        },
        execution_progress: ExecutionProgressSnapshot {
            consumed_steps: 123,
            original_limits: crate::api::ExecutionLimits::new(
                std::num::NonZeroU32::new(64).expect("non-zero test call depth"),
                Some(std::num::NonZeroU64::new(900).expect("non-zero test step limit")),
            )
            .expect("valid test execution limits"),
        },
        host_context: HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow_with_args(
                    "Memory",
                    "write",
                    vec![ActionArgPattern::Prefix(vec![
                        "project".to_owned(),
                        "cache".to_owned(),
                    ])],
                )],
                approvals: vec![ApprovalGrant {
                    id: etas_host::HostRequestId(77),
                    grants: vec![HostActionGrant::Allow(ActionPattern::Exact(
                        ActionInstance::new(
                            "Tool",
                            "invoke",
                            vec![HostValue::String("lookup".to_owned())],
                        ),
                    ))],
                }],
                sandbox: SandboxPolicy::allow_listed(
                    FilesystemPolicy {
                        read_roots: vec![WorkspaceRoot {
                            canonical_root: PathBuf::from("/tmp/etas-read"),
                        }],
                        write_roots: vec![WorkspaceRoot {
                            canonical_root: PathBuf::from("/tmp/etas-write"),
                        }],
                        delete_roots: Vec::new(),
                    },
                    NetworkPolicy {
                        allowed_endpoints: vec![NetworkEndpoint::new("https", "example.com", 443)],
                    },
                    CommandPolicy::allow_programs(vec!["git".to_owned()]),
                    DestructiveOpPolicy::deny_all(),
                ),
                policy: PolicyContext {
                    active_trace_specs: vec!["RuntimeGate".to_owned()],
                    trace_spec_facts: vec![HostValue::Record(vec![
                        (
                            "kind".to_owned(),
                            HostValue::String("policy_reference".to_owned()),
                        ),
                        (
                            "name".to_owned(),
                            HostValue::String("RuntimeGate".to_owned()),
                        ),
                    ])],
                    labels: vec!["phase1".to_owned()],
                    boundary_policy_ref: Some(HostValue::Record(vec![(
                        "policy".to_owned(),
                        HostValue::String("runtime".to_owned()),
                    )])),
                },
            },
            trace: TraceContext {
                trace_id: TraceId(55),
                parent_span: Some(TraceSpanId(56)),
            },
            budget: etas_host::ExecutionBudget::start(Budget {
                tokens: Some(TokenBudget { max_tokens: 4096 }),
                time: Some(TimeBudget { max_millis: 10_000 }),
                cost: Some(CostBudget {
                    max_micros: 250,
                    currency: "USD".to_owned(),
                }),
            }),
        },
        current_session: Some("session-42".to_owned()),
        resource_versions: ResourceVersionSnapshot {
            versions: vec![ResourceVersionRecord {
                resource: "memory:main".to_owned(),
                version: "v2".to_owned(),
            }],
        },
        completed_host_boundaries: HostBoundaryLedger {
            completed: vec![CompletedHostBoundary {
                kind: "console".to_owned(),
                key: "read:line".to_owned(),
                result: CompletedHostBoundaryResult::Runtime(InterpValue::String("ok".to_owned())),
            }],
        },
    };

    let artifact = checkpoint_artifact_json(&[PathBuf::from("main.es")], "main", &checkpoint)
        .expect("checkpoint artifact should encode");
    let restored =
        checkpoint_from_json(&artifact, &checked).expect("checkpoint codec should restore");

    assert_eq!(restored.handlers, checkpoint.handlers);
    assert_eq!(restored.host_context, checkpoint.host_context);
    assert_eq!(restored.current_session, checkpoint.current_session);
    assert_eq!(restored.trace, checkpoint.trace);
    assert_eq!(restored.execution_progress, checkpoint.execution_progress);
    assert_eq!(restored.resource_versions, checkpoint.resource_versions);
    assert_eq!(
        restored.completed_host_boundaries,
        checkpoint.completed_host_boundaries
    );

    let changed_checked = crate::testing::project::checked_project(
        r#"
module app.main;

effect Gate {
  action request(value: string) -> string;
}

flow main() -> unit {
  let value = perform Gate.request("changed") with {
    Gate.request(value) => resume value;
  };
  return;
}
"#,
    );
    let error = checkpoint_from_json(&artifact, &changed_checked)
        .expect_err("checkpoint from changed source must fail closed");
    assert!(error.message().contains("project fingerprint"));

    for (field, expected_message) in [
        ("compiler_version", "compiler version"),
        ("checked_hir_fingerprint", "checked-HIR fingerprint"),
        (
            "dependency_metadata_fingerprints",
            "dependency metadata fingerprints",
        ),
        ("entry_semantic_identity", "entry semantic identity"),
    ] {
        let mut invalid_identity = artifact.clone();
        invalid_identity["checkpoint"]["compilation"][field] =
            if field == "dependency_metadata_fingerprints" {
                json!([{"package": "dep", "fingerprint": "changed"}])
            } else {
                json!("changed")
            };
        let error = checkpoint_from_json(&invalid_identity, &checked)
            .expect_err("checkpoint with mismatched compilation identity must fail closed");
        assert!(
            error.message().contains(expected_message),
            "unexpected identity error for {field}: {}",
            error.message()
        );
    }

    let mut invalid_item = artifact.clone();
    invalid_item["checkpoint"]["entry_item"] = json!(u32::MAX);
    let error = checkpoint_from_json(&invalid_item, &checked)
        .expect_err("checkpoint with an invalid HIR item must fail closed");
    assert!(
        error
            .message()
            .contains("does not exist in the checked HIR")
    );

    let mut mismatched_scope = artifact.clone();
    mismatched_scope["checkpoint"]["handlers"][0]["id"] = json!(1);
    let error = checkpoint_from_json(&mismatched_scope, &checked)
        .expect_err("handler stack and boundary scope mismatch must fail closed");
    assert!(error.message().contains("handler scope topology mismatch"));

    let mut duplicate_scope = artifact.clone();
    let handler = duplicate_scope["checkpoint"]["handlers"][0].clone();
    duplicate_scope["checkpoint"]["handlers"] = json!([handler.clone(), handler]);
    let error = checkpoint_from_json(&duplicate_scope, &checked)
        .expect_err("duplicate active handler scope must fail closed");
    assert!(error.message().contains("handler scope 0 is duplicated"));

    let mut reversed_scopes = artifact.clone();
    let mut outer = reversed_scopes["checkpoint"]["handlers"][0].clone();
    outer["id"] = json!(2);
    let mut inner = reversed_scopes["checkpoint"]["handlers"][0].clone();
    inner["id"] = json!(1);
    reversed_scopes["checkpoint"]["handlers"] = json!([outer, inner]);
    let error = checkpoint_from_json(&reversed_scopes, &checked)
        .expect_err("reversed active handler scope order must fail closed");
    assert!(error.message().contains("handler scope order is invalid"));

    let mut duplicate_boundary = artifact.clone();
    let boundary = duplicate_boundary["checkpoint"]["machine"]["frames"][0].clone();
    duplicate_boundary["checkpoint"]["machine"]["frames"] = json!([boundary.clone(), boundary]);
    let error = checkpoint_from_json(&duplicate_boundary, &checked)
        .expect_err("duplicate handler boundaries must fail closed");
    assert!(
        error
            .message()
            .contains("duplicate handle boundary scope 0")
    );

    let mut wrong_boundary_order = artifact.clone();
    let mut handler_zero = wrong_boundary_order["checkpoint"]["handlers"][0].clone();
    handler_zero["id"] = json!(0);
    let mut handler_one = handler_zero.clone();
    handler_one["id"] = json!(1);
    wrong_boundary_order["checkpoint"]["handlers"] = json!([handler_zero, handler_one]);
    let mut boundary_zero = wrong_boundary_order["checkpoint"]["machine"]["frames"][0].clone();
    boundary_zero["continuation"]["scope_id"] = json!(0);
    let mut boundary_one = boundary_zero.clone();
    boundary_one["continuation"]["scope_id"] = json!(1);
    wrong_boundary_order["checkpoint"]["machine"]["frames"] = json!([boundary_one, boundary_zero]);
    let error = checkpoint_from_json(&wrong_boundary_order, &checked)
        .expect_err("same handler IDs in the wrong unwind order must fail closed");
    assert!(error.message().contains("expected unwind order"));

    let mut invalid_handler = artifact;
    invalid_handler["checkpoint"]["machine"]["frames"] = json!([{
        "kind": "handler",
        "continuation": { "kind": "block_value" },
    }]);
    let error = checkpoint_from_json(&invalid_handler, &checked)
        .expect_err("checkpoint with mismatched handler frame nesting must fail closed");
    assert!(
        error
            .message()
            .contains("does not contain a handler boundary")
    );
}

#[test]
fn checkpoint_codec_rejects_legacy_artifact_without_machine_stack() {
    let artifact = json!({
        "schema": "etas.cli.interpreter-checkpoint.v1",
        "checkpoint": {},
    });
    let error = checkpoint_from_json(&artifact, &checked_project())
        .expect_err("legacy checkpoint must fail closed");
    assert!(
        error
            .message()
            .contains("expected `etas.cli.interpreter-checkpoint.v11`")
    );
}

#[test]
fn checkpoint_codec_rejects_v4_artifact_after_handler_scope_schema_change() {
    let checked = checked_project();
    let entry = checked.entry.expect("test entry");
    let compilation = CheckpointCompilationIdentity::for_project(&checked, entry)
        .expect("test compilation identity");
    let artifact = json!({
        "schema": "etas.cli.interpreter-checkpoint.v4",
        "checkpoint": {
            "id": 0,
            "label": null,
            "compilation": compilation_identity_json(&compilation),
            "entry_item": entry.0,
            "args": [],
            "continuation": { "kind": "block_value" },
        },
    });
    let error = checkpoint_from_json(&artifact, &checked)
        .expect_err("v4 checkpoint must be rejected by schema version");
    assert!(
        error.message().contains(
            "unsupported checkpoint artifact schema `etas.cli.interpreter-checkpoint.v4`; expected `etas.cli.interpreter-checkpoint.v11`"
        ),
        "{}",
        error.message()
    );
}

#[test]
fn checkpoint_codec_rejects_v5_artifact_after_lossless_host_ledger_schema_change() {
    let artifact = json!({
        "schema": "etas.cli.interpreter-checkpoint.v5",
        "checkpoint": {},
    });
    let error = checkpoint_from_json(&artifact, &checked_project())
        .expect_err("v5 checkpoint must be rejected by schema version");
    assert!(
        error.message().contains(
            "unsupported checkpoint artifact schema `etas.cli.interpreter-checkpoint.v5`; expected `etas.cli.interpreter-checkpoint.v11`"
        ),
        "{}",
        error.message()
    );
}

#[test]
fn checkpoint_codec_rejects_v6_artifact_after_canonical_message_schema_change() {
    let artifact = json!({
        "schema": "etas.cli.interpreter-checkpoint.v6",
        "checkpoint": {},
    });
    let error = checkpoint_from_json(&artifact, &checked_project())
        .expect_err("v6 checkpoint must be rejected by schema version");
    assert!(
        error.message().contains(
            "unsupported checkpoint artifact schema `etas.cli.interpreter-checkpoint.v6`; expected `etas.cli.interpreter-checkpoint.v11`"
        ),
        "{}",
        error.message()
    );
}

#[test]
fn checkpoint_codec_rejects_v7_artifact_without_checked_intrinsic_abi() {
    let artifact = json!({
        "schema": "etas.cli.interpreter-checkpoint.v7",
        "checkpoint": {},
    });
    let error = checkpoint_from_json(&artifact, &checked_project())
        .expect_err("v7 checkpoint must be rejected by schema version");
    assert!(
        error.message().contains(
            "unsupported checkpoint artifact schema `etas.cli.interpreter-checkpoint.v7`; expected `etas.cli.interpreter-checkpoint.v11`"
        ),
        "{}",
        error.message()
    );
}

#[test]
fn checkpoint_codec_rejects_v8_artifact_with_executable_std_callable_payloads() {
    let artifact = json!({
        "schema": "etas.cli.interpreter-checkpoint.v8",
        "checkpoint": {},
    });
    let error = checkpoint_from_json(&artifact, &checked_project())
        .expect_err("v8 checkpoint must be rejected by schema version");
    assert!(
        error.message().contains(
            "unsupported checkpoint artifact schema `etas.cli.interpreter-checkpoint.v8`; expected `etas.cli.interpreter-checkpoint.v11`"
        ),
        "{}",
        error.message()
    );
}

#[test]
fn checkpoint_codec_rejects_v9_artifact_without_run_owned_budget_state() {
    let artifact = json!({
        "schema": "etas.cli.interpreter-checkpoint.v9",
        "checkpoint": {},
    });
    let error = checkpoint_from_json(&artifact, &checked_project())
        .expect_err("v9 checkpoint must be rejected by schema version");
    assert!(
        error.message().contains(
            "unsupported checkpoint artifact schema `etas.cli.interpreter-checkpoint.v9`; expected `etas.cli.interpreter-checkpoint.v11`"
        ),
        "{}",
        error.message()
    );
}

#[test]
fn checkpoint_codec_rejects_v10_artifact_without_execution_progress() {
    let artifact = json!({
        "schema": "etas.cli.interpreter-checkpoint.v10",
        "checkpoint": {},
    });
    let error = checkpoint_from_json(&artifact, &checked_project())
        .expect_err("v10 checkpoint must be rejected by schema version");
    assert!(
        error.message().contains(
            "unsupported checkpoint artifact schema `etas.cli.interpreter-checkpoint.v10`; expected `etas.cli.interpreter-checkpoint.v11`"
        ),
        "{}",
        error.message()
    );
}

#[test]
fn checkpoint_codec_rejects_unimported_and_unknown_std_intrinsics() {
    let checked = checked_project();
    for (intrinsic, dispatch) in [
        (etas_std::intrinsic::runtime::FS_READ_BYTES, "host"),
        (etas_std::intrinsic::runtime::NET_TCP_CONNECT, "host"),
        (u32::MAX, "runtime"),
    ] {
        let mut artifact = minimal_checkpoint_artifact(&checked);
        artifact["checkpoint"]["args"] = json!([{
            "kind": "callable",
            "target": {
                "kind": "std_intrinsic",
                "intrinsic": intrinsic,
                "dispatch": dispatch,
                "parameter_types": [],
                "result_type": 0,
            },
        }]);
        let error = checkpoint_from_json(&artifact, &checked)
            .expect_err("checkpoint must not inject an intrinsic outside the current plan");
        assert!(
            error.message().contains("is not imported"),
            "unexpected error for intrinsic {intrinsic}: {}",
            error.message()
        );
    }
}

#[test]
fn checkpoint_codec_rejects_imported_intrinsic_dispatch_category_tampering() {
    let checked = crate::testing::project::checked_project(
        r#"
module app.main;

import std.io.println;

flow main() -> unit ![Console.stdout_write, Error<IOError>] {
  println("ok");
  return;
}
"#,
    );
    let mut artifact = minimal_checkpoint_artifact(&checked);
    artifact["checkpoint"]["args"] = json!([{
        "kind": "callable",
        "target": {
            "kind": "std_intrinsic",
            "intrinsic": etas_std::intrinsic::runtime::IO_PRINTLN,
            "dispatch": "host",
            "parameter_types": [],
            "result_type": 0,
        },
    }]);
    let error = checkpoint_from_json(&artifact, &checked)
        .expect_err("checkpoint dispatch category tampering must fail closed");
    assert!(error.message().contains("dispatch mismatch"), "{error:?}");
}

#[test]
fn checkpoint_codec_rejects_pure_kernel_encoded_as_runtime_std_target() {
    let checked = crate::testing::project::checked_project(
        r#"
module app.main;

import std.option.unwrap as option_unwrap;

flow main() -> i32 {
  let value: Option<i32> = Some(7);
  return option_unwrap(value);
}
"#,
    );
    let mut artifact = minimal_checkpoint_artifact(&checked);
    artifact["checkpoint"]["args"] = json!([{
        "kind": "callable",
        "target": {
            "kind": "std_intrinsic",
            "intrinsic": etas_std::intrinsic::pure::OPTION_UNWRAP,
            "dispatch": "pure_kernel",
            "parameter_types": [],
            "result_type": 0,
        },
    }]);
    let error = checkpoint_from_json(&artifact, &checked)
        .expect_err("pure kernels must use the checked pure intrinsic checkpoint ABI");
    assert!(
        error
            .message()
            .contains("is a pure kernel and requires checked ABI facts"),
        "{error:?}"
    );
}

#[test]
fn run_report_json_includes_message_session_trace_events() {
    let report = run_report_json(
        "run",
        &[PathBuf::from("main.es")],
        "main",
        &RunResult {
            value: None,
            diagnostics: Vec::new(),
            events: vec![
                WorkflowEvent::MessageCreated {
                    id: "msg-0".to_owned(),
                    from: None,
                    to: None,
                    session: Some("session-42".to_owned()),
                    role: "user".to_owned(),
                    created_at: "step-1".to_owned(),
                    payload: Box::new(InterpValue::String("hello".to_owned())),
                    provenance: Some(crate::value::ProvenanceValue {
                        trace_id: Some("TraceId(0)".to_owned()),
                        source: Some("Message.new".to_owned()),
                    }),
                },
                WorkflowEvent::MessageSessionAttached {
                    id: "msg-0".to_owned(),
                    session: "session-43".to_owned(),
                    session_config: crate::value::SessionConfigValue {
                        id: "session-43".to_owned(),
                        context: Some(Box::new(InterpValue::Variant {
                            name: "LastTurns".to_owned(),
                            fields: vec![InterpValue::i32(4)],
                        })),
                        retention: None,
                        compaction: None,
                    },
                },
                WorkflowEvent::MessageHandoff {
                    id: "msg-0".to_owned(),
                    from: Some("customer".to_owned()),
                    to: Some("triage".to_owned()),
                    session: Some("session-43".to_owned()),
                    target_item: 7,
                },
                WorkflowEvent::SessionResolved {
                    session: "session-43".to_owned(),
                    created: true,
                },
                WorkflowEvent::SessionMessageAppended {
                    session: "session-43".to_owned(),
                    message: "msg-0".to_owned(),
                    deduplicated: false,
                },
                WorkflowEvent::SessionHistoryLoaded {
                    session: "session-43".to_owned(),
                    message_count: 1,
                    has_summary: true,
                    cursor: Some("1".to_owned()),
                },
                WorkflowEvent::SessionCompacted {
                    session: "session-43".to_owned(),
                    summary_message_count: 1,
                },
            ],
            checkpoints: Vec::new(),
        },
    )
    .expect("run report should encode");
    assert_eq!(
        report["events"][0],
        json!({
            "kind": "message_created",
            "id": "msg-0",
            "from": null,
            "to": null,
            "session": "session-42",
            "role": "user",
            "created_at": "step-1",
            "payload": {
                "kind": "string",
                "value": "hello",
            },
            "provenance": {
                "trace_id": "TraceId(0)",
                "source": "Message.new",
            },
        })
    );
    assert_eq!(
        report["events"][1],
        json!({
            "kind": "message_session_attached",
            "id": "msg-0",
            "session": "session-43",
            "session_config": {
                "id": "session-43",
                "context": {
                    "kind": "variant",
                    "name": "LastTurns",
                    "fields": [
                        {
                            "kind": "number",
                            "type": "i32",
                            "value": "4",
                        },
                    ],
                },
                "retention": null,
                "compaction": null,
            },
        })
    );
    assert_eq!(
        report["events"][2],
        json!({
            "kind": "message_handoff",
            "id": "msg-0",
            "from": "customer",
            "to": "triage",
            "session": "session-43",
            "target_item": 7,
        })
    );
    assert_eq!(
        report["events"][3],
        json!({
            "kind": "session_resolved",
            "session": "session-43",
            "created": true,
        })
    );
    assert_eq!(
        report["events"][4],
        json!({
            "kind": "session_message_appended",
            "session": "session-43",
            "message": "msg-0",
            "deduplicated": false,
        })
    );
    assert_eq!(
        report["events"][5],
        json!({
            "kind": "session_history_loaded",
            "session": "session-43",
            "message_count": 1,
            "has_summary": true,
            "cursor": "1",
        })
    );
    assert_eq!(
        report["events"][6],
        json!({
            "kind": "session_compacted",
            "session": "session-43",
            "summary_message_count": 1,
        })
    );
}

#[test]
fn run_report_json_preserves_structured_host_trace_events() {
    let metadata = etas_host::HostTraceMetadata::for_action(
        "memory",
        "Memory.get",
        &etas_host::HostTraceDigestKey::from_bytes([7; 32]),
    )
    .expect("trace metadata should be valid");
    let payload_digest = metadata.payload_digest.clone();
    let report = run_report_json(
        "run",
        &[PathBuf::from("main.es")],
        "main",
        &RunResult {
            value: None,
            diagnostics: Vec::new(),
            events: vec![
                WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted {
                    id: HostRequestId(7),
                    kind: etas_host::HostRequestKind::Memory,
                    metadata,
                    authority: Box::new(AuthorityContext::deny_all()),
                    trace: TraceContext {
                        trace_id: TraceId(11),
                        parent_span: Some(TraceSpanId(12)),
                    },
                    started_at_unix_micros: 100,
                }),
                WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestFinished {
                    id: HostRequestId(7),
                    outcome: etas_host::HostOutcome::Failed(etas_host::HostError::new(
                        etas_host::HostErrorCode::ProviderUnavailable,
                        "memory offline",
                    )),
                    finished_at_unix_micros: 125,
                    duration_micros: 25,
                }),
            ],
            checkpoints: Vec::new(),
        },
    )
    .expect("structured host trace should encode");

    assert_eq!(
        report["events"][0],
        json!({
            "kind": "host_request_started",
            "id": 7,
            "request_kind": "memory",
            "qualified_action": "Memory.get",
            "subject_kind": "memory",
            "payload": [],
            "payload_digest": payload_digest,
            "started_at_unix_micros": 100,
            "trace": { "trace_id": 11, "parent_span": 12 },
            "authority": {
                "grant_count": 0,
                "approval_count": 0,
                "active_trace_specs": [],
            },
        })
    );
    assert_eq!(
        report["events"][1],
        json!({
            "kind": "host_request_finished",
            "id": 7,
            "finished_at_unix_micros": 125,
            "duration_micros": 25,
            "outcome": {
                "kind": "failed",
                "code": "ProviderUnavailable",
                "message": "memory offline",
                "details": [],
            },
        })
    );
}

#[test]
fn value_codec_round_trips_canonical_message_contract() {
    let expected = InterpValue::Message(crate::value::MessageValue {
        id: "msg-7".to_owned(),
        from: Some("user".to_owned()),
        to: Some("agent".to_owned()),
        role: crate::value::MessageRoleValue::User,
        session: Some("session-7".to_owned()),
        created_at: "step-4".to_owned(),
        payload: Box::new(InterpValue::String("hello".to_owned())),
        provenance: Some(crate::value::ProvenanceValue {
            trace_id: Some("trace-7".to_owned()),
            source: Some("test".to_owned()),
        }),
    });

    let encoded = value_json(&expected);
    let restored = value_from_json(&encoded).expect("message codec should restore");

    assert_eq!(restored, expected);
}

#[test]
fn value_codec_rejects_v7_message_missing_or_unknown_contract_fields() {
    let base = json!({
        "kind": "message",
        "id": "msg-1",
        "from": null,
        "to": null,
        "role": "user",
        "session": null,
        "created_at": "step-1",
        "payload": { "kind": "string", "value": "hello" },
        "provenance": null,
    });

    for field in [
        "id",
        "from",
        "to",
        "role",
        "session",
        "created_at",
        "payload",
        "provenance",
    ] {
        let mut damaged = base.clone();
        damaged
            .as_object_mut()
            .expect("message fixture should be an object")
            .remove(field);
        let error = value_from_json(&damaged).expect_err("missing message field must fail closed");
        assert!(error.message().contains(field), "{error:?}");
    }

    let mut damaged = base;
    damaged
        .as_object_mut()
        .expect("message fixture should be an object")
        .insert("session_config".to_owned(), serde_json::Value::Null);
    let error = value_from_json(&damaged).expect_err("unknown message field must fail closed");
    assert!(error.message().contains("session_config"), "{error:?}");
}

#[test]
fn value_codec_rejects_missing_v6_collection_payloads() {
    for damaged in [
        json!({ "kind": "map" }),
        json!({ "kind": "record" }),
        json!({ "kind": "prompt" }),
        json!({ "kind": "handler", "fact_expr": 0 }),
    ] {
        value_from_json(&damaged)
            .expect_err("missing checkpoint collection payload must fail closed");
    }

    for damaged in [json!({ "kind": "map" }), json!({ "kind": "record" })] {
        host_value_from_json(&damaged)
            .expect_err("missing lossless host ledger payload must fail closed");
    }
}

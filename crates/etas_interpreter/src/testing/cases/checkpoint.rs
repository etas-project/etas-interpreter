use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_continues_from_saved_state() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

flow main() -> string {
  checkpoint("pause");
  return "done";
}
"#,
    );

    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let checkpoint = first.checkpoints.first().expect("checkpoint record");

    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            checkpoint,
            &FakeHost::new(HostServiceAvailability::with_host(
                HostRequirementKind::Checkpoint,
            )),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        resumed.value().cloned(),
        Some(value::InterpValue::String("done".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_budget_snapshot_does_not_change_after_later_token_consumption() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.checkpoint;

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow main() -> string {
  checkpoint("before-model");
  let value = Writer.run("hello");
  checkpoint("after-model");
  return value;
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
    ]));
    host.seed_model_response_text("done");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: vec![HostActionGrant::allow("Agentic", "infer")],
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: Default::default(),
                    },
                    trace: TraceContext::root(TraceId(97)),
                    budget: etas_host::ExecutionBudget::start(Budget {
                        tokens: Some(TokenBudget { max_tokens: 10 }),
                        ..Budget::default()
                    }),
                },
                ..RunOptions::default()
            },
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.checkpoints.len(), 2);
    assert_eq!(
        result.checkpoints[0]
            .host_state
            .budget
            .state
            .consumed_tokens,
        0
    );
    assert!(
        result.checkpoints[1]
            .host_state
            .budget
            .state
            .consumed_tokens
            > 0
    );
    assert_eq!(
        result.checkpoints[0]
            .host_state
            .budget
            .state
            .consumed_tokens,
        0,
        "later model usage must not mutate an earlier checkpoint snapshot"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_preserves_consumed_execution_fuel_and_cannot_widen_limits() {
    let mut source = String::from(
        r#"
module app.main;
import std.runtime.checkpoint;

flow main() -> i32 {
    checkpoint("fuel");
"#,
    );
    for index in 0..200 {
        source.push_str(&format!("    let value_{index} = {index};\n"));
    }
    source.push_str("    return 7;\n}\n");
    let checked = checked_project(&source);
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let original_limits = crate::api::ExecutionLimits::new(
        std::num::NonZeroU32::new(128).expect("non-zero test call depth"),
        Some(std::num::NonZeroU64::new(100).expect("non-zero test fuel")),
    )
    .expect("valid test execution limits");
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions {
                execution_limits: original_limits,
                ..RunOptions::default()
            },
        )
        .await
        .expect("execution lifecycle infrastructure");
    let checkpoint = first
        .checkpoints
        .first()
        .expect("execution should create a checkpoint before exhausting fuel");
    assert!(checkpoint.execution_progress.consumed_steps > 0);
    assert_eq!(
        checkpoint.execution_progress.original_limits,
        original_limits
    );

    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            checkpoint,
            &host,
            RunOptions {
                execution_limits: crate::api::ExecutionLimits::default(),
                ..RunOptions::default()
            },
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(resumed.value().cloned().is_none());
    assert!(resumed.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("maximum interpreter execution steps (100) exceeded")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn public_resume_rejects_injected_std_intrinsic_outside_checked_plan() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

flow main() -> unit {
  checkpoint("pause");
  return;
}
"#,
    );
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

    let mut checkpoint = first
        .checkpoints
        .first()
        .expect("checkpoint record")
        .clone();
    checkpoint.args.push(value::InterpValue::Callable(
        crate::control::CallTarget::StdIntrinsic(
            crate::intrinsic::dispatch::CheckedStdIntrinsicCall {
                identity: crate::intrinsic::dispatch::StdIntrinsicIdentity {
                    intrinsic: etas_std::StdIntrinsicId(
                        etas_std::intrinsic::runtime::NET_TCP_CONNECT,
                    ),
                    dispatch: etas_std::IntrinsicDispatch::Host,
                },
                parameter_types: Vec::new(),
                result_type: etas_types::TypeId(0),
            },
        ),
    ));

    let rejected = Interpreter
        .resume_checkpoint(
            &checked,
            &checkpoint,
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(rejected.value().cloned().is_none());
    assert!(
        rejected.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("is not imported by the current checked interpreter plan")
        }),
        "{:?}",
        rejected.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_artifact_restores_deep_non_tail_call_stack() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

flow descend(n: i32) -> i32 {
  if n == 0 {
    checkpoint("deep-call-stack");
    return 0;
  }
  return 1 + descend(n - 1);
}

flow main() -> i32 {
  return descend(1000);
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let checkpoint = first.checkpoints.first().expect("checkpoint record");
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &[std::path::PathBuf::from("main.es")],
        "main",
        checkpoint,
    )
    .expect("deep machine checkpoint artifact should encode");
    let restored = crate::api::codec::checkpoint_from_json(&artifact, &checked)
        .expect("deep machine checkpoint artifact must decode");

    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &restored,
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        resumed.value().cloned(),
        Some(value::InterpValue::i32(1000))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resumed_deep_call_stack_executes_host_boundary_after_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.println;
import std.runtime.{checkpoint};

flow descend(n: i32) -> i32 ![Console, Error<IOError>] {
  if n == 0 {
    checkpoint("before-console");
    println("deep");
    return 0;
  }
  return 1 + descend(n - 1);
}

flow main() -> i32 ![Console, Error<IOError>] {
  return descend(1000);
}
"#,
    );
    let requirements = [
        HostRequirementKind::Checkpoint,
        HostRequirementKind::Console,
    ];
    let first_host = FakeHost::new(availability(&requirements));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first_host.console_call_count(), 1);
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &[std::path::PathBuf::from("main.es")],
        "main",
        first.checkpoints.first().expect("checkpoint record"),
    )
    .expect("deep host checkpoint artifact should encode");
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked)
        .expect("deep host checkpoint artifact must decode");
    let resumed_host = FakeHost::new(availability(&requirements));
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &resumed_host, RunOptions::default())
        .await
        .expect("execution lifecycle infrastructure");

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        resumed.value().cloned(),
        Some(value::InterpValue::i32(1000))
    );
    assert_eq!(resumed_host.console_call_count(), 1);
    assert_eq!(resumed_host.stdout_text(), "deep\n");
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_one_thousand_calls_through_scoped_handler_stack() {
    let checked = checked_project(
        r#"
module app.main;

effect Gate {
  action request() -> i32;
}

flow descend(n: i32) -> i32 {
  if n == 0 {
    return perform Gate.request();
  }
  return descend(n - 1) with {
    Gate.request() => {
      resume 1;
    }
  };
}

flow main() -> i32 {
  return descend(1000);
}
"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value().cloned(), Some(value::InterpValue::i32(1)));
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_artifact_restores_nested_scoped_handler_stack() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

effect Gate {
  action request() -> i32;
}

flow descend(n: i32) -> i32 {
  if n == 0 {
    checkpoint("inside-handler-stack");
    return perform Gate.request();
  }
  return descend(n - 1) with {
    Gate.request() => {
      resume 7;
    }
  };
}

flow main() -> i32 {
  return descend(128);
}
"#,
    );
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let checkpoint = first.checkpoints.first().expect("checkpoint record");
    assert!(
        checkpoint.machine.frames.iter().any(|frame| matches!(
            frame,
            crate::orchestration::MachineFrameSnapshot::Handler { .. }
        )),
        "checkpoint must store active handler boundaries as explicit machine frames"
    );
    let mut invalid_checkpoint = checkpoint.clone();
    invalid_checkpoint
        .handlers
        .handlers
        .last_mut()
        .expect("checkpoint must contain an active handler")
        .id = crate::orchestration::HandlerScopeId(u32::MAX);
    let rejected = Interpreter
        .resume_checkpoint(
            &checked,
            &invalid_checkpoint,
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");
    assert!(rejected.value().cloned().is_none());
    assert!(
        rejected.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("checkpoint state validation failed")),
        "{:?}",
        rejected.diagnostics
    );
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &[std::path::PathBuf::from("main.es")],
        "main",
        checkpoint,
    )
    .expect("nested handler checkpoint artifact should encode");
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked)
        .expect("nested handler checkpoint artifact must decode");
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &checkpoint,
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value().cloned(), Some(value::InterpValue::i32(7)));
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_artifact_restores_nested_retry_stack() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};
import std.runtime.limits.Attempts;

flow descend(n: i32) -> i32 {
  if n == 0 {
    checkpoint("inside-retry-stack");
    return 0;
  }
  retry limit Attempts(1) {
    return 1 + descend(n - 1);
  }
  return -1;
}

flow main() -> i32 {
  return descend(128);
}
"#,
    );
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let checkpoint = first.checkpoints.first().expect("checkpoint record");
    assert_eq!(checkpoint.retry_state.attempts.len(), 128);
    assert!(
        checkpoint.machine.frames.iter().any(|frame| matches!(
            frame,
            crate::orchestration::MachineFrameSnapshot::Retry { .. }
        )),
        "checkpoint must store active retry attempts as explicit machine frames"
    );
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &[std::path::PathBuf::from("main.es")],
        "main",
        checkpoint,
    )
    .expect("nested retry checkpoint artifact should encode");
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked)
        .expect("nested retry checkpoint artifact must decode");
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &checkpoint,
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value().cloned(), Some(value::InterpValue::i32(128)));
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_artifact_restores_captured_lambda_local() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

flow main() -> i32 {
  let offset = 1;
  let add_offset = (value: i32) => value + offset;
  checkpoint("with-lambda");
  return add_offset(41);
}
"#,
    );
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &[std::path::PathBuf::from("main.es")],
        "main",
        first.checkpoints.first().expect("checkpoint record"),
    )
    .expect("lambda checkpoint artifact should encode");
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked)
        .expect("lambda checkpoint artifact must decode");
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &checkpoint,
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value().cloned(), Some(value::InterpValue::i32(42)));
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_restores_handler_context() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

effect Approval {
  action request(value: string) -> string;
}

flow main(input: string) -> string {
  let result = handle {
    checkpoint("pause-in-handler");
    let approved = perform Approval.request(input);
    approved
  } with {
    Approval.request(value) => {
      resume value;
    }
  };

  return result;
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Approval,
        HostRequirementKind::Checkpoint,
    ]));

    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("approved".to_owned())],
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let checkpoint = first.checkpoints.first().expect("checkpoint record");
    assert_eq!(checkpoint.handlers.handlers.len(), 1);
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &[std::path::PathBuf::from("main.es")],
        "main",
        checkpoint,
    )
    .expect("handler checkpoint artifact should encode");
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked)
        .expect("handler checkpoint artifact must decode");

    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &checkpoint,
            &FakeHost::new(availability(&[
                HostRequirementKind::Approval,
                HostRequirementKind::Checkpoint,
            ])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        resumed.value().cloned(),
        Some(value::InterpValue::String("approved".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_keeps_monotonic_checkpoint_ids() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

flow main() -> string {
  checkpoint("first");
  checkpoint("second");
  return "done";
}
"#,
    );

    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let checkpoint = first.checkpoints.first().expect("checkpoint record");
    assert_eq!(checkpoint.id.0, 0);

    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            checkpoint,
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    let resumed_checkpoint = resumed
        .checkpoints
        .first()
        .expect("resumed checkpoint record");
    assert_eq!(resumed_checkpoint.id.0, 1);
    assert_eq!(
        resumed.value().cloned(),
        Some(value::InterpValue::String("done".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_preserves_completed_host_boundary_ledger() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

flow main() -> string {
  checkpoint("first");
  checkpoint("second");
  return "done";
}
"#,
    );

    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let mut checkpoint = first
        .checkpoints
        .first()
        .expect("checkpoint record")
        .clone();
    checkpoint
        .completed_host_boundaries
        .completed
        .push(CompletedHostBoundary {
            occurrence: crate::orchestration::BoundaryOccurrenceId::HostRequest(
                etas_host::HostRequestId(checkpoint.trace.next_host_request),
            ),
            kind: "approval".to_owned(),
            key: "req-1".to_owned(),
            result: crate::orchestration::CompletedHostBoundaryResult::Runtime(
                value::InterpValue::Bool(true),
            ),
        });

    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &checkpoint,
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    let resumed_checkpoint = resumed
        .checkpoints
        .first()
        .expect("resumed checkpoint record");
    assert_eq!(
        resumed_checkpoint.completed_host_boundaries.completed.len(),
        1
    );
    assert_eq!(
        resumed_checkpoint.completed_host_boundaries.completed[0].kind,
        "approval"
    );
    assert_eq!(
        resumed_checkpoint.completed_host_boundaries.completed[0].key,
        "req-1"
    );
}

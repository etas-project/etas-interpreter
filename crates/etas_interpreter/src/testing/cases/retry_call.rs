use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn run_checked_snapshots_retry_state_inside_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};
import std.runtime.limits.Attempts;

flow main() -> unit {
  retry limit Attempts(2) {
    checkpoint("during-retry");
  }

  return;
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
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.checkpoints.len(), 1);
    assert_eq!(result.checkpoints[0].retry_state.attempts.len(), 1);
    assert_eq!(result.checkpoints[0].retry_state.attempts[0].ordinal, 0);

    let artifact = crate::api::codec::checkpoint_artifact_json(
        &[std::path::PathBuf::from("main.es")],
        "main",
        &result.checkpoints[0],
    )
    .expect("retry checkpoint artifact should encode");
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked)
        .expect("retry checkpoint artifact must decode");
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &checkpoint,
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await;

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value, Some(value::InterpValue::Unit));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_retries_retryable_model_boundary_failure() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.limits.Attempts;

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow main() -> string {
  retry limit Attempts(2) {
    return Writer.run("draft");
  }

  return "unreachable";
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_error(HostErrorCode::ProviderUnavailable, "transient model outage");
    host.seed_model_response_text("recovered");

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
                    ..Default::default()
                },
                model_policy: crate::api::ModelExecutionPolicy {
                    provider: Some(ModelProviderId("fixture".to_owned())),
                    model: ModelName("fixture-model".to_owned()),
                    provider_capabilities: Some(full_model_capabilities()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("recovered".to_owned()))
    );
    assert_eq!(host.model_call_count(), 2);
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::RetryAttemptFailed(_)))
    );
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::RetryAttemptSucceeded(_)))
    );
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::RetryExhausted))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_collapses_suspended_retrying_callee_return_to_call_value() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.limits.Attempts;

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow helper() -> string {
  var plan = "";
  retry limit Attempts(2) {
    plan = Writer.run("draft");
  }
  return plan;
}

flow main() -> string {
  let value = helper();
  return value + ":after";
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("recovered");

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
                    ..Default::default()
                },
                model_policy: crate::api::ModelExecutionPolicy {
                    provider: Some(ModelProviderId("fixture".to_owned())),
                    model: ModelName("fixture-model".to_owned()),
                    provider_capabilities: Some(full_model_capabilities()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("recovered:after".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_continues_caller_after_callee_console_boundary_before_record_return() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.io.println;
import std.runtime.limits.Attempts;

type Review = {
  summary: string,
};

agent Writer(input: string) -> Review {
  return Prompt.new().user(Public(input));
}

flow helper() -> Review ![Error<IOError>] {
  var seed = "";
  retry limit Attempts(2) {
    seed = "ready";
  }
  let review = Writer.run(seed);
  println(review.summary);
  return review;
}

flow main() -> string ![Error<IOError>] {
  let first = helper();
  println("after");
  return first.summary + ":done";
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Console,
    ]));
    host.seed_model_response_text(r#"{"summary":"reviewed"}"#);

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
                        grants: vec![
                            HostActionGrant::allow("Agentic", "infer"),
                            HostActionGrant::allow("Console", "stdout_write"),
                        ],
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: Default::default(),
                    },
                    ..Default::default()
                },
                model_policy: crate::api::ModelExecutionPolicy {
                    provider: Some(ModelProviderId("fixture".to_owned())),
                    model: ModelName("fixture-model".to_owned()),
                    provider_capabilities: Some(full_model_capabilities()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("reviewed:done".to_owned()))
    );
    assert_eq!(host.stdout_text(), "reviewed\nafter\n");
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_default_call_depth_allows_twenty_four_recursive_calls() {
    let checked = checked_project(
        r#"
module app.main;

flow recurse(value: i32) -> i32 {
  if value >= 24 {
    return value;
  }
  return recurse(value + 1);
}

flow main() -> i32 {
  return recurse(0);
}
"#,
    );

    let host = FakeHost::new(HostServiceAvailability::default());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(24)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_call_depth_exhaustion_instead_of_stack_overflow() {
    let checked = checked_project(
        r#"
module app.main;

flow recurse(value: i32) -> i32 {
  return recurse(value + 1);
}

flow main() -> i32 {
  return recurse(0);
}
"#,
    );

    let host = FakeHost::new(HostServiceAvailability::default());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions {
                execution_limits: api::ExecutionLimits {
                    max_call_depth: std::num::NonZeroU32::new(32).expect("non-zero"),
                    max_steps: None,
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert_eq!(result.value, None);
    let terminal = result
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .message
                .contains("maximum interpreter call depth (32)")
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal.len(), 1, "{:#?}", result.diagnostics);
    assert_eq!(
        terminal[0].code,
        DiagnosticCode::Analysis(AnalysisDiagnosticCode::UnhandledRuntimeError)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_default_call_depth_exhaustion_is_a_diagnostic() {
    let checked = checked_project(
        r#"
module app.main;

flow recurse(value: i32) -> i32 {
  return 1 + recurse(value + 1);
}

flow main() -> i32 {
  return recurse(0);
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
        .await;

    assert_eq!(result.value, None);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::UnhandledRuntimeError)
            && diagnostic
                .message
                .contains("maximum interpreter call depth (4096)")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_one_thousand_non_tail_recursive_calls() {
    assert_non_tail_recursion_completes(1_000, api::ExecutionLimits::default()).await;
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_five_thousand_non_tail_recursive_calls() {
    assert_non_tail_recursion_completes(
        5_000,
        api::ExecutionLimits::new(std::num::NonZeroU32::new(6_000).expect("non-zero"), None)
            .expect("test call depth is below the hard cap"),
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_ten_thousand_non_tail_recursive_calls() {
    assert_non_tail_recursion_completes(
        10_000,
        api::ExecutionLimits::new(std::num::NonZeroU32::new(11_000).expect("non-zero"), None)
            .expect("test call depth is below the hard cap"),
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_twenty_thousand_non_tail_recursive_calls() {
    assert_non_tail_recursion_completes(
        20_000,
        api::ExecutionLimits::new(std::num::NonZeroU32::new(21_000).expect("non-zero"), None)
            .expect("test call depth is below the hard cap"),
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn call_budget_work_scales_near_linearly_with_depth() {
    let measure = |depth| async move {
        let started = std::time::Instant::now();
        assert_non_tail_recursion_completes(
            depth,
            api::ExecutionLimits::new(
                std::num::NonZeroU32::new(depth as u32 + 1_000).expect("non-zero"),
                None,
            )
            .expect("test call depth is below the hard cap"),
        )
        .await;
        started.elapsed()
    };

    let five_thousand = measure(5_000).await;
    let ten_thousand = measure(10_000).await;
    let twenty_thousand = measure(20_000).await;
    let tolerance = std::time::Duration::from_millis(500);
    assert!(
        ten_thousand <= five_thousand.saturating_mul(3) + tolerance,
        "doubling call depth must remain near-linear: 5k={five_thousand:?}, 10k={ten_thousand:?}"
    );
    assert!(
        twenty_thousand <= ten_thousand.saturating_mul(3) + tolerance,
        "doubling call depth must remain near-linear: 10k={ten_thousand:?}, 20k={twenty_thousand:?}"
    );
}

async fn assert_non_tail_recursion_completes(depth: i32, limits: api::ExecutionLimits) {
    let checked = checked_project(
        r#"
module app.main;

flow recurse(value: i32) -> i32 {
  if value == 0 {
    return 0;
  }
  return 1 + recurse(value - 1);
}

flow main(value: i32) -> i32 {
  return recurse(value);
}
"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::i32(depth)],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions {
                execution_limits: limits,
                ..RunOptions::default()
            },
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(depth)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_execution_step_exhaustion() {
    let checked = checked_project(
        r#"
module app.main;

flow recurse(value: i32) -> i32 {
  return recurse(value + 1);
}

flow main() -> i32 {
  return recurse(0);
}
"#,
    );

    let host = FakeHost::new(HostServiceAvailability::default());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions {
                execution_limits: api::ExecutionLimits {
                    max_call_depth: api::ExecutionLimits::default().max_call_depth,
                    max_steps: Some(std::num::NonZeroU64::new(50).expect("non-zero")),
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert_eq!(result.value, None);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::UnhandledRuntimeError)
            && diagnostic
                .message
                .contains("maximum interpreter execution steps (50)")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_retries_retryable_host_tool_failure_inside_model_loop() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.limits.Attempts;

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow main() -> string {
  retry limit Attempts(2) {
    return Writer.run("draft");
  }

  return "unreachable";
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::ToolCall,
    ]));
    host.seed_model_response_tool_call("Search");
    host.seed_tool_error(HostErrorCode::ProviderUnavailable, "transient tool outage");
    host.seed_model_response_tool_call("Search");
    host.seed_tool_response_once(HostValue::String("tool-result".to_owned()));
    host.seed_model_response_text("recovered through tool");

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
                    ..Default::default()
                },
                model_policy: crate::api::ModelExecutionPolicy {
                    provider: Some(ModelProviderId("fixture".to_owned())),
                    model: ModelName("fixture-model".to_owned()),
                    provider_capabilities: Some(full_model_capabilities()),
                    tools: vec![ToolSchema {
                        tool: ToolRef::anonymous_test("Search"),
                        input: HostSchema::Record(Vec::new()),
                        output: Some(HostSchema::String),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String(
            "recovered through tool".to_owned()
        ))
    );
    assert_eq!(host.model_call_count(), 3);
    assert_eq!(host.tool_requests().len(), 2);
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::RetryAttemptFailed(_)))
    );
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::RetryAttemptSucceeded(_)))
    );
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::RetryExhausted))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_retries_typed_memory_conflict_error() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.limits.Attempts;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> string ![Memory.write, Error<MemoryConflict>] {
  retry limit Attempts(2) {
    ProjectMemory.Papers.put("paper-1", "draft");
    return "written";
  }

  return "unreachable";
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.seed_memory_conflict(etas_host::MemoryConflict {
        expected: Some(etas_host::MemoryVersion {
            opaque: "v0".to_owned(),
        }),
        actual: Some(etas_host::MemoryVersion {
            opaque: "v1".to_owned(),
        }),
        current_value: Some(HostValue::String("existing".to_owned())),
    });

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("written".to_owned()))
    );
    assert_eq!(host.memory_call_count(), 2);
    assert_eq!(
        host.memory_value(
            "project_memory",
            &["Papers"],
            &HostValue::String("paper-1".to_owned())
        ),
        Some(HostValue::String("draft".to_owned()))
    );
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::RetryAttemptFailed(_)))
    );
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::RetryAttemptSucceeded(_)))
    );
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::RetryExhausted))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_exhausts_retry_after_repeated_typed_memory_conflicts() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.limits.Attempts;

alias ProjectMemorySchema = MemoryRegion<{
  Papers: Store<string, string>
}>;

let ProjectMemory =
  std.memory.region<ProjectMemorySchema>(
    stable_id = "project_memory",
    store = "project-main"
  );

flow main() -> string ![Memory.write, Error<MemoryConflict>] {
  retry limit Attempts(2) {
    ProjectMemory.Papers.put("paper-1", "draft");
    return "written";
  }

  return "unreachable";
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    for version in ["v1", "v2"] {
        host.seed_memory_conflict(etas_host::MemoryConflict {
            expected: Some(etas_host::MemoryVersion {
                opaque: "v0".to_owned(),
            }),
            actual: Some(etas_host::MemoryVersion {
                opaque: version.to_owned(),
            }),
            current_value: Some(HostValue::String("existing".to_owned())),
        });
    }

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert_eq!(result.value, None);
    assert_eq!(host.memory_call_count(), 2);
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, WorkflowEvent::RetryExhausted))
    );
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("retry exhausted after 2 attempt(s)")
            && diagnostic.message.contains("MemoryConflict")
    }));
}

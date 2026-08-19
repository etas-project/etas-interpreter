use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_agent_model_tool_call_loop() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

tool Search() -> string {
  return "local";
}

@model(model = "local-qwen")
@tools([Search])
agent Writer(input: string) -> string ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_tool_call("Search");
    host.seed_model_response_text("final answer");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(46)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            options,
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("final answer".to_owned()))
    );
    assert_eq!(host.model_call_count(), 2);
    assert_eq!(
        host.tool_requests().len(),
        0,
        "source-bodied tool calls should execute checked HIR directly"
    );
    let requests = host.model_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].tools,
        vec![ToolSchema {
            tool: ToolRef::source("Search", "app.main.Search"),
            input: HostSchema::Record(Vec::new()),
            output: Some(HostSchema::String),
        }]
    );
    assert!(requests[1].messages.iter().any(|message| {
        message.role == etas_host::ModelRole::Assistant && !message.tool_calls.is_empty()
    }));
    assert!(requests[1].messages.iter().any(|message| {
        message.role == etas_host::ModelRole::Tool
            && message.tool_call_id.as_deref() == Some("call-1")
            && message.content.iter().any(
                |content| matches!(content, ModelContent::Value(HostValue::String(value)) if value == "local")
            )
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_external_package_tool_from_metadata_schema() {
    let tool_path = vec!["dep".to_owned(), "tools".to_owned(), "Search".to_owned()];
    let checked = checked_project_with_environment(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import dep.tools.Search;

@model(model = "local-qwen")
@tools([Search])
agent Writer(input: string) -> string ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
        external_search_tool_environment(true),
    );

    assert_eq!(checked.external_tool_schemas.len(), 1);
    assert_eq!(checked.external_tool_schemas[0].path, tool_path);

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::ToolCall,
    ]));
    host.seed_model_response_tool_call_with_args(
        "Search",
        HostValue::Record(vec![(
            "query".to_owned(),
            HostValue::String("hello".to_owned()),
        )]),
    );
    host.seed_tool_response_once(HostValue::String("external result".to_owned()));
    host.seed_model_response_text("final answer");

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: vec![HostActionGrant::allow("Agentic", "infer")],
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: Default::default(),
                    },
                    trace: TraceContext::root(TraceId(66)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                model_policy: api::ModelExecutionPolicy {
                    provider_capabilities: Some(full_model_capabilities()),
                    ..api::ModelExecutionPolicy::default()
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("final answer".to_owned()))
    );
    assert_eq!(host.model_call_count(), 2);
    assert_eq!(host.tool_requests().len(), 1);

    let requests = host.model_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].tools,
        vec![ToolSchema {
            tool: ToolRef::external("Search", "dep.tools.Search"),
            input: HostSchema::Record(vec![HostFieldSchema {
                name: "query".to_owned(),
                schema: HostSchema::String,
                optional: false,
            }]),
            output: Some(HostSchema::String),
        }]
    );
    assert_eq!(host.tool_requests()[0].tool.name, "Search");
    assert_eq!(
        host.tool_requests()[0].tool.qualified_name.as_deref(),
        Some("dep.tools.Search")
    );
    assert_eq!(
        host.tool_requests()[0].args,
        HostValue::Record(vec![(
            "query".to_owned(),
            HostValue::String("hello".to_owned()),
        )])
    );
    assert!(requests[1].messages.iter().any(|message| {
        message.role == etas_host::ModelRole::Tool
            && message.tool_call_id.as_deref() == Some("call-1")
            && message.content.iter().any(
                |content| matches!(content, ModelContent::Value(HostValue::String(value)) if value == "external result")
            )
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_external_package_tool_without_metadata_schema() {
    let checked = checked_project_with_environment(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import dep.tools.Search;

@model(model = "local-qwen")
@tools([Search])
agent Writer(input: string) -> string ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
        external_search_tool_environment(false),
    );

    assert!(checked.external_tool_schemas.is_empty());

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::ToolCall,
    ]));
    host.seed_model_response_tool_call_with_args(
        "Search",
        HostValue::Record(vec![(
            "query".to_owned(),
            HostValue::String("hello".to_owned()),
        )]),
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: vec![HostActionGrant::allow("Agentic", "infer")],
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: Default::default(),
                    },
                    trace: TraceContext::root(TraceId(67)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                model_policy: api::ModelExecutionPolicy {
                    provider_capabilities: Some(full_model_capabilities()),
                    ..api::ModelExecutionPolicy::default()
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert_eq!(result.value, None);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingCheckedFact)
            && diagnostic
                .message
                .contains("missing package tool schema metadata")
    }));
    assert_eq!(host.model_call_count(), 0);
    assert_eq!(host.tool_requests().len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_records_completed_host_tool_boundary_in_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.{checkpoint};

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  let answer = Writer.run(input);
  checkpoint("after-tool-loop");
  return answer;
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::ToolCall,
        HostRequirementKind::Checkpoint,
    ]));
    host.seed_model_response_tool_call("Search");
    host.seed_tool_response_once(HostValue::String("tool-result".to_owned()));
    host.seed_model_response_text("final answer");

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: vec![HostActionGrant::allow("Agentic", "infer")],
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: Default::default(),
                    },
                    trace: TraceContext::root(TraceId(63)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                model_policy: api::ModelExecutionPolicy {
                    provider_capabilities: Some(full_model_capabilities()),
                    tools: vec![ToolSchema {
                        tool: ToolRef::anonymous_test("Search"),
                        input: HostSchema::Record(Vec::new()),
                        output: Some(HostSchema::String),
                    }],
                    ..api::ModelExecutionPolicy::default()
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("final answer".to_owned()))
    );
    assert_eq!(host.model_call_count(), 2);
    assert_eq!(host.tool_requests().len(), 1);
    let checkpoint = result.checkpoints.first().expect("checkpoint snapshot");
    assert!(
        checkpoint
            .completed_host_boundaries
            .completed
            .iter()
            .any(|boundary| boundary.kind == "tool" && boundary.key.contains("tool:Search"))
    );
    assert!(
        checkpoint
            .completed_host_boundaries
            .completed
            .iter()
            .any(|boundary| boundary.kind == "model")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_returns_invalid_tool_args_to_model_for_repair() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

tool Search() -> string {
  return "local";
}

@model(model = "local-qwen")
@tools([Search])
agent Writer(input: string) -> string ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_tool_call_with_args(
        "Search",
        HostValue::Record(vec![(
            "query".to_owned(),
            HostValue::String("extra".to_owned()),
        )]),
    );
    host.seed_model_response_tool_call("Search");
    host.seed_model_response_text("final answer");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(60)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            options,
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("final answer".to_owned()))
    );
    assert_eq!(host.model_call_count(), 3);
    assert_eq!(host.tool_requests().len(), 0);
    let model_requests = host.model_requests();
    assert!(
        model_requests
            .get(1)
            .is_some_and(|request| request.messages.iter().any(|message| {
                message.role == ModelRole::Tool
                    && message.content.iter().any(|content| {
                        matches!(
                            content,
                            ModelContent::Value(HostValue::Record(fields))
                                if fields.iter().any(|(name, value)| {
                                    name == "kind"
                                        && matches!(
                                            value,
                                            HostValue::String(kind)
                                                if kind == "InvalidToolArguments"
                                        )
                                })
                        )
                    })
            })),
        "{model_requests:#?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_applies_policy_gate_before_host_tool_call() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::ToolCall,
    ]));
    host.seed_policy_decision(PolicyDecision::Allow);
    host.seed_policy_decision(PolicyDecision::Deny {
        reason: "tool denied".to_owned(),
    });
    host.seed_model_response_tool_call("Search");

    let policy_ref = HostValue::Variant {
        name: "LastTurns".to_owned(),
        fields: vec![HostValue::Int(8)],
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: vec![HostActionGrant::allow("Agentic", "infer")],
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: Default::default(),
                    },
                    trace: TraceContext::root(TraceId(61)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                model_policy: api::ModelExecutionPolicy {
                    provider_capabilities: Some(full_model_capabilities()),
                    policy_ref: Some(policy_ref.clone()),
                    tools: vec![ToolSchema {
                        tool: ToolRef::anonymous_test("Search"),
                        input: HostSchema::Record(Vec::new()),
                        output: Some(HostSchema::String),
                    }],
                    ..api::ModelExecutionPolicy::default()
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert_eq!(result.value, None);
    assert_eq!(host.model_call_count(), 1);
    assert_eq!(host.policy_call_count(), 2);
    assert_eq!(
        host.tool_requests().len(),
        0,
        "policy denial must stop host tool invocation"
    );
    let policy_requests = host.policy_requests();
    assert_eq!(policy_requests.len(), 2);
    assert_eq!(policy_requests[0].policy_ref, policy_ref);
    assert_eq!(policy_requests[0].subject.kind, "model");
    assert_eq!(policy_requests[1].subject.kind, "tool");
    assert!(policy_requests[1].subject.attributes.iter().any(
        |(name, value)| name == "tool" && value == &HostValue::String("Search".to_owned())
    ));
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("tool policy denied request: tool denied")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_applies_policy_gate_before_source_tool_call() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

tool Search() -> string {
  return "local";
}

@model(model = "local-qwen")
@tools([Search])
agent Writer(input: string) -> string ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_policy_decision(PolicyDecision::Allow);
    host.seed_policy_decision(PolicyDecision::Deny {
        reason: "source tool denied".to_owned(),
    });
    host.seed_model_response_tool_call("Search");

    let policy_ref = HostValue::String("source-tool-policy".to_owned());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: vec![HostActionGrant::allow("Agentic", "infer")],
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: Default::default(),
                    },
                    trace: TraceContext::root(TraceId(63)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                model_policy: api::ModelExecutionPolicy {
                    provider_capabilities: Some(full_model_capabilities()),
                    policy_ref: Some(policy_ref.clone()),
                    ..api::ModelExecutionPolicy::default()
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert_eq!(result.value, None);
    assert_eq!(host.model_call_count(), 1);
    assert_eq!(host.policy_call_count(), 2);
    assert_eq!(
        host.tool_requests().len(),
        0,
        "source-bodied tool policy denial must not fall through to host tool invocation"
    );
    let policy_requests = host.policy_requests();
    assert_eq!(policy_requests.len(), 2);
    assert_eq!(policy_requests[1].policy_ref, policy_ref);
    assert_eq!(policy_requests[1].subject.kind, "tool");
    assert!(policy_requests[1].subject.attributes.iter().any(
        |(name, value)| name == "tool" && value == &HostValue::String("Search".to_owned())
    ));
    assert!(
        policy_requests[1]
            .subject
            .attributes
            .iter()
            .any(|(name, value)| name == "source_item" && matches!(value, HostValue::String(_)))
    );
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("tool policy denied request: source tool denied")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn source_tool_checkpoint_roundtrip_resumes_model_loop_and_preserves_tool_call_id() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.{checkpoint};

type Draft = { text: string };

tool Search() -> string {
  checkpoint("inside-source-tool");
  return "local-result";
}

@model(model = "local-qwen")
@tools([Search])
agent Writer(input: string) -> Draft ![] {
  return Prompt.new().user(Public(input));
}

flow descend(depth: i32, input: string) -> Draft {
  if depth == 0 {
    return Writer.run(input);
  }
  return descend(depth - 1, input);
}

flow main(input: string) -> Draft {
  return descend(24, input);
}
"#,
    );
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(64)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
    ]));
    first_host.seed_model_response_text("not valid typed json");
    first_host.seed_model_response_tool_call("Search");
    first_host.seed_model_response_text(r#"{"text":"first-completion"}"#);
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &first_host,
            options.clone(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let checkpoint = first
        .checkpoints
        .iter()
        .find(|checkpoint| checkpoint.label.as_deref() == Some("inside-source-tool"))
        .expect("source tool checkpoint");
    let source_tool_frame = checkpoint
        .machine
        .frames
        .iter()
        .find_map(|frame| match frame {
            crate::orchestration::MachineFrameSnapshot::SourceToolReturn(frame) => Some(frame),
            _ => None,
        })
        .expect("source tool return frame");
    assert_eq!(source_tool_frame.tool_call_id, "call-1");
    assert_eq!(source_tool_frame.model_loop.round, 2);
    assert_eq!(source_tool_frame.model_loop.repair.attempts, 1);
    let active_calls = checkpoint
        .machine
        .frames
        .iter()
        .filter(|frame| {
            matches!(
                frame,
                crate::orchestration::MachineFrameSnapshot::Call { .. }
            )
        })
        .count();
    assert_eq!(
        active_calls, 28,
        "the machine must retain main->descend recursion (25), agent (1), source-tool (1), and checkpoint intrinsic (1) call frames"
    );
    let artifact = crate::api::codec::checkpoint_artifact_json(
        &[std::path::PathBuf::from("main.es")],
        "main",
        checkpoint,
    )
    .expect("source tool checkpoint artifact should encode");
    let restored = crate::api::codec::checkpoint_from_json(&artifact, &checked)
        .expect("source tool machine checkpoint must decode");
    let restored_source_tool_frame = restored
        .machine
        .frames
        .iter()
        .find_map(|frame| match frame {
            crate::orchestration::MachineFrameSnapshot::SourceToolReturn(frame) => Some(frame),
            _ => None,
        })
        .expect("restored source tool return frame");
    assert_eq!(restored_source_tool_frame.tool_call_id, "call-1");
    assert_eq!(restored_source_tool_frame.model_loop.round, 2);
    assert_eq!(restored_source_tool_frame.model_loop.repair.attempts, 1);

    let resumed_host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
    ]));
    resumed_host.seed_model_response_text(r#"{"text":"resumed-completion"}"#);
    let resumed = Interpreter
        .resume_checkpoint(&checked, &restored, &resumed_host, options)
        .await;

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    let Some(value::InterpValue::Nominal { value, .. }) = resumed.value else {
        panic!("resumed typed model output must preserve nominal runtime identity");
    };
    assert_eq!(
        *value,
        value::InterpValue::Record(
            vec![(
                "text".to_owned(),
                value::InterpValue::String("resumed-completion".to_owned()),
            )]
            .into(),
        )
    );
    let requests = resumed_host.model_requests();
    assert_eq!(requests.len(), 1, "{requests:#?}");
    assert!(requests[0].messages.iter().any(|message| {
        message.role == ModelRole::Tool
            && message.tool_call_id.as_deref() == Some("call-1")
            && message.content
                == vec![ModelContent::Value(HostValue::String(
                    "local-result".to_owned(),
                ))]
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn source_tool_handler_retry_checkpoint_roundtrip_restores_all_machine_boundaries() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.{checkpoint};
import std.runtime.limits.Attempts;

effect Gate {
  action request() -> string;
}

tool Search() -> string ![Gate] {
  retry limit Attempts(2) {
    return perform Gate.request() with {
      Gate.request() => {
        checkpoint("inside-source-handler-retry");
        resume "local-result";
      }
    };
  }
  return abort("retry should return");
}

@model(model = "local-qwen")
@tools([Search])
agent Writer(input: string) -> string ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(65)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
    ]));
    first_host.seed_model_response_tool_call("Search");
    first_host.seed_model_response_text("first-completion");
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &first_host,
            options.clone(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let checkpoint = first
        .checkpoints
        .iter()
        .find(|checkpoint| checkpoint.label.as_deref() == Some("inside-source-handler-retry"))
        .expect("nested source tool checkpoint");
    assert!(checkpoint.machine.frames.iter().any(|frame| matches!(
        frame,
        crate::orchestration::MachineFrameSnapshot::SourceToolReturn(_)
    )));
    assert!(checkpoint.machine.frames.iter().any(|frame| matches!(
        frame,
        crate::orchestration::MachineFrameSnapshot::Retry { .. }
    )));
    assert!(checkpoint.machine.frames.iter().any(|frame| matches!(
        frame,
        crate::orchestration::MachineFrameSnapshot::Handler { .. }
    )));

    let artifact = crate::api::codec::checkpoint_artifact_json(
        &[std::path::PathBuf::from("main.es")],
        "main",
        checkpoint,
    )
    .expect("nested source tool checkpoint artifact should encode");
    let restored = crate::api::codec::checkpoint_from_json(&artifact, &checked)
        .expect("nested source tool checkpoint must decode");
    let resumed_host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
    ]));
    resumed_host.seed_model_response_text("resumed-completion");
    let resumed = Interpreter
        .resume_checkpoint(&checked, &restored, &resumed_host, options)
        .await;

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        resumed.value,
        Some(value::InterpValue::String("resumed-completion".to_owned()))
    );
    assert_eq!(resumed_host.model_call_count(), 1);
    assert_eq!(resumed_host.tool_requests().len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_stops_host_tool_call_when_policy_approval_is_denied() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::ToolCall,
        HostRequirementKind::Approval,
    ]));
    host.seed_policy_decision(PolicyDecision::Allow);
    host.seed_policy_decision(PolicyDecision::RequireApproval {
        request: ApprovalRequest {
            id: HostRequestId(902),
            reason: "tool requires approval".to_owned(),
            requested_grants: Vec::new(),
            trace: TraceContext::root(TraceId(62)),
        },
    });
    host.seed_approval_decision(ApprovalDecision::Denied {
        reason: "operator denied".to_owned(),
    });
    host.seed_model_response_tool_call("Search");

    let policy_ref = HostValue::String("tool-approval-policy".to_owned());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: vec![HostActionGrant::allow("Agentic", "infer")],
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: Default::default(),
                    },
                    trace: TraceContext::root(TraceId(62)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                model_policy: api::ModelExecutionPolicy {
                    provider_capabilities: Some(full_model_capabilities()),
                    policy_ref: Some(policy_ref.clone()),
                    tools: vec![ToolSchema {
                        tool: ToolRef::anonymous_test("Search"),
                        input: HostSchema::Record(Vec::new()),
                        output: Some(HostSchema::String),
                    }],
                    ..api::ModelExecutionPolicy::default()
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert_eq!(result.value, None);
    assert_eq!(host.model_call_count(), 1);
    assert_eq!(host.policy_call_count(), 2);
    assert_eq!(host.approval_call_count(), 1);
    assert_eq!(
        host.tool_requests().len(),
        0,
        "approval denial must stop host tool invocation"
    );
    let policy_requests = host.policy_requests();
    assert_eq!(policy_requests.len(), 2);
    let tool_subject = &policy_requests[1].subject;
    assert_eq!(policy_requests[1].policy_ref, policy_ref);
    assert_eq!(tool_subject.kind, "tool");
    assert!(tool_subject.attributes.iter().any(|(name, value)| {
        name == "qualified_action" && value == &HostValue::String("Tool.call".to_owned())
    }));
    assert!(tool_subject.attributes.iter().any(
        |(name, value)| name == "resource" && value == &HostValue::String("Search".to_owned())
    ));
    assert!(
        tool_subject
            .attributes
            .iter()
            .any(|(name, value)| name == "trace_id"
                && value == &HostValue::String("TraceId(62)".to_owned()))
    );
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("tool policy approval was denied")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_requires_tool_call_host_support_for_runtime_configured_tools() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_tool_call("Search");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: vec![HostActionGrant::allow("Agentic", "infer")],
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: Default::default(),
                    },
                    trace: TraceContext::root(TraceId(62)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                model_policy: api::ModelExecutionPolicy {
                    provider_capabilities: Some(full_model_capabilities()),
                    tools: vec![ToolSchema {
                        tool: ToolRef::anonymous_test("Search"),
                        input: HostSchema::Record(Vec::new()),
                        output: Some(HostSchema::String),
                    }],
                    ..api::ModelExecutionPolicy::default()
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert_eq!(result.value, None);
    assert_eq!(host.model_call_count(), 1);
    assert_eq!(host.tool_requests().len(), 0);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingHostHandler)
            && diagnostic.message.contains("ToolCall support")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_prompt_data_encodes_record_input_as_json() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

type AgentRequest = {
  topic: string,
  retries: i32,
};

agent Writer(input: AgentRequest) -> string {
  return Prompt.new().system(Trusted("write concise output")).data(input);
}

flow main(topic: string) -> string {
  return Writer.run(AgentRequest { topic = topic, retries = 2 });
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("draft");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("storage".to_owned())],
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let requests = host.model_requests();
    let request = requests.first().expect("model request should be sent");
    let data_message = request
        .messages
        .iter()
        .find(|message| message.role == ModelRole::User)
        .expect("data prompt should lower to user model message");
    assert_eq!(
        data_message.content,
        vec![ModelContent::Text(
            r#"{"retries":2,"topic":"storage"}"#.to_owned()
        )]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_agent_policy_delegates_to_host_policy_client() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("draft");
    let policy_ref = HostValue::Variant {
        name: "LastTurns".to_owned(),
        fields: vec![HostValue::Int(8)],
    };
    let options = RunOptions {
        model_policy: api::ModelExecutionPolicy {
            policy_ref: Some(policy_ref.clone()),
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            options,
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.policy_call_count(), 1);
    let policy_requests = host.policy_requests();
    let policy_request = policy_requests
        .first()
        .expect("agent policy should delegate to host policy client");
    assert_eq!(policy_request.policy_ref, policy_ref);
    assert_eq!(policy_request.subject.kind, "model");
    let model_requests = host.model_requests();
    assert_eq!(model_requests.len(), 1);
    assert_eq!(
        model_requests[0].policy_ref,
        Some(policy_request.policy_ref.clone())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_agent_policy_denial_stops_model_request() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.deny_policy("test deny");
    let options = RunOptions {
        model_policy: api::ModelExecutionPolicy {
            policy_ref: Some(HostValue::Variant {
                name: "LastTurns".to_owned(),
                fields: vec![HostValue::Int(8)],
            }),
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            options,
        )
        .await;

    assert_eq!(result.value, None);
    assert_eq!(host.policy_call_count(), 1);
    assert_eq!(host.model_call_count(), 0);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("model policy denied request: test deny")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_can_return_raw_model_response_support_value() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> ModelResponse {
  return Prompt.new().system(Trusted("write concise output")).user(Public(input));
}

flow main(input: string) -> ModelResponse {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("draft");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(43)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            response_decode: api::ModelResponseDecodePolicy::ModelResponse,
            ..api::ModelExecutionPolicy::default()
        },
        ..RunOptions::default()
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            options,
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::ModelResponse(
            value::ModelResponseValue {
                id: 0,
                message: value::ModelMessageValue {
                    role: value::ModelRoleValue::Assistant,
                    content: vec![value::ModelContentValue::Text("draft".to_owned())],
                },
                tool_calls: Vec::new(),
                usage: Some(value::ModelUsageValue {
                    input_tokens: 1,
                    output_tokens: 1,
                }),
            }
        ))
    );
}

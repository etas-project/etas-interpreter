use super::super::*;

#[test]
fn plan_collects_host_requirements_for_agent_entry() {
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

    let plan = Interpreter.plan(&checked, PlanOptions).plan.unwrap();
    let Some(InterpreterSupport::RequiresHost(requirements)) = plan.host_requirements.entry else {
        panic!("expected host requirement for agent inference");
    };
    assert!(requirements.kinds.contains(&HostRequirementKind::Agentic));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_missing_host_handler_for_reachable_host_requirement() {
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

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hi".to_owned())],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingHostHandler)
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_spec_method_selection() {
    let checked = checked_project(
        r#"
module app.main;

spec PromptEncode {
  flow encode(input: string) -> string;
}

impl string ~ PromptEncode {
  flow encode(input: string) -> string {
    return "encoded";
  }
}

flow main(input: string) -> string {
  return input::PromptEncode.encode();
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("hi".to_owned())],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("encoded".to_owned())),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_agent_call_through_model_host_service() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return Prompt.new().system(Trusted("write concise output")).user(Public(input));
}

flow main(input: string) -> string {
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
            trace: TraceContext::root(TraceId(42)),
            budget: etas_host::ExecutionBudget::start(Budget {
                tokens: Some(TokenBudget { max_tokens: 128 }),
                ..Budget::default()
            }),
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
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("draft".to_owned()))
    );
    assert_eq!(host.model_call_count(), 1);

    let requests = host.model_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].messages.len(), 2);
    assert_eq!(requests[0].messages[0].role, ModelRole::System);
    assert_eq!(
        requests[0].messages[0].content,
        vec![ModelContent::Text("write concise output".to_owned())]
    );
    assert_eq!(requests[0].messages[1].role, ModelRole::User);
    assert_eq!(
        requests[0].messages[1].content,
        vec![ModelContent::Text("hello".to_owned())]
    );
    assert_eq!(
        requests[0].authority.grants,
        vec![HostActionGrant::allow("Agentic", "infer")]
    );
    assert_eq!(requests[0].trace, TraceContext::root(TraceId(42)));
    assert_eq!(
        requests[0].budget.limits().tokens,
        Some(TokenBudget { max_tokens: 128 })
    );
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted {
            id: HostRequestId(0),
            kind: etas_host::HostRequestKind::Model,
            metadata,
            authority,
            trace,
            started_at_unix_micros,
        }) if authority.grants == vec![HostActionGrant::allow("Agentic", "infer")]
            && trace == &TraceContext::root(TraceId(42))
            && metadata.qualified_action == "Agentic.infer"
            && metadata.payload_digest.len() == 64
            && *started_at_unix_micros > 0
    )));
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestFinished {
            id: HostRequestId(0),
            outcome: etas_host::HostOutcome::Succeeded,
            finished_at_unix_micros,
            ..
        }) if *finished_at_unix_micros > 0
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn model_response_id_mismatch_fails_closed_and_preserves_trace_pairing() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow main() -> string {
  return Writer.run("hello");
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("draft");
    host.force_next_model_response_id(HostRequestId(99));
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
                    trace: TraceContext::root(TraceId(91)),
                    budget: etas_host::ExecutionBudget::start(Budget::default()),
                },
                ..RunOptions::default()
            },
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.value().cloned().is_none());
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("host response id does not match the originating request id")
    }));
    let trace_ids = result
        .events
        .iter()
        .filter_map(|event| match event {
            WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted { id, .. })
            | WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestFinished { id, .. }) => {
                Some(*id)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(trace_ids, vec![HostRequestId(0), HostRequestId(0)]);
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestFinished {
            outcome: etas_host::HostOutcome::Failed(error),
            ..
        }) if error.code == HostErrorCode::InvalidResponse
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn run_owned_token_budget_is_consumed_across_model_calls() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow main() -> string {
  let first = Writer.run("first");
  return Writer.run(first);
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("one");
    host.seed_model_response_text("two");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(43)),
            budget: etas_host::ExecutionBudget::start(Budget {
                tokens: Some(TokenBudget { max_tokens: 3 }),
                ..Budget::default()
            }),
        },
        ..RunOptions::default()
    };

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.value().cloned().is_none());
    assert_eq!(host.model_call_count(), 2);
    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("run-owned execution token budget is exhausted")
        }),
        "{:?}",
        result.diagnostics
    );
    let snapshot = host.model_requests()[0]
        .budget
        .snapshot()
        .expect("shared budget snapshot");
    assert_eq!(snapshot.consumed_tokens, 2);
    assert_eq!(snapshot.reserved_tokens, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn token_budget_fails_closed_when_model_omits_usage() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow main() -> string {
  return Writer.run("first");
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text_with_usage("one", None);
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(44)),
            budget: etas_host::ExecutionBudget::start(Budget {
                tokens: Some(TokenBudget { max_tokens: 10 }),
                ..Budget::default()
            }),
        },
        ..RunOptions::default()
    };

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.value().cloned().is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("model response omitted usage required by the run-owned execution budget")
        }),
        "{:?}",
        result.diagnostics
    );
    let snapshot = host.model_requests()[0]
        .budget
        .snapshot()
        .expect("shared budget snapshot");
    assert_eq!(snapshot.consumed_tokens, 0);
    assert_eq!(snapshot.reserved_tokens, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn model_cost_usage_is_settled_into_the_run_owned_budget() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

agent Writer(input: string) -> string {
  return Prompt.new().user(Public(input));
}

flow main() -> string {
  return Writer.run("first");
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text_with_usage(
        "one",
        Some(etas_host::ModelUsage {
            input_tokens: 1,
            output_tokens: 1,
            cost: Some(etas_host::ModelCostUsage {
                micros: 30,
                currency: "USD".to_owned(),
            }),
        }),
    );
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(45)),
            budget: etas_host::ExecutionBudget::start(Budget {
                cost: Some(etas_host::CostBudget {
                    max_micros: 100,
                    currency: "USD".to_owned(),
                }),
                ..Budget::default()
            }),
        },
        ..RunOptions::default()
    };

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            options,
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("one".to_owned())),
        "{:?}",
        result.diagnostics
    );
    let snapshot = host.model_requests()[0]
        .budget
        .snapshot()
        .expect("shared budget snapshot");
    assert_eq!(snapshot.consumed_cost_micros, 30);
    assert_eq!(snapshot.reserved_cost_micros, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resumes_agent_prompt_body_after_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.io.read_line;

agent Writer() -> string ![Error<IOError>] {
  let line = read_line();
  return Prompt.new().user(Public(line));
}

flow main() -> string ![Error<IOError>] {
  return Writer.run();
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Console,
    ]));
    host.seed_stdin("from stdin\n");
    host.seed_model_response_text("model answer");

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
                    trace: TraceContext::root(TraceId(52)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                ..RunOptions::default()
            },
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("model answer".to_owned()))
    );
    assert_eq!(host.console_call_count(), 1);
    assert_eq!(host.model_call_count(), 1);

    let requests = host.model_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].messages.len(), 1);
    assert_eq!(requests[0].messages[0].role, ModelRole::User);
    assert_eq!(
        requests[0].messages[0].content,
        vec![ModelContent::Text("from stdin\n".to_owned())]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resumes_prompt_method_argument_after_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.io.read_line;

agent Writer() -> string ![Error<IOError>] {
  return Prompt.new().user(Public(read_line()));
}

flow main() -> string ![Error<IOError>] {
  return Writer.run();
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Console,
    ]));
    host.seed_stdin("nested stdin\n");
    host.seed_model_response_text("nested answer");

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
                    trace: TraceContext::root(TraceId(53)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                ..RunOptions::default()
            },
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::String("nested answer".to_owned()))
    );
    assert_eq!(host.console_call_count(), 1);
    assert_eq!(host.model_call_count(), 1);

    let requests = host.model_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].messages.len(), 1);
    assert_eq!(requests[0].messages[0].role, ModelRole::User);
    assert_eq!(
        requests[0].messages[0].content,
        vec![ModelContent::Text("nested stdin\n".to_owned())]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resumes_local_method_argument_after_host_boundary() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.read_line;

flow main() -> bool ![Error<IOError>] {
  let empty: Array<string> = [];
  let values = empty.push(read_line());
  return values.is_empty();
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    host.seed_stdin("from method\n");

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
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(value::InterpValue::Bool(false))
    );
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_applies_agent_model_tools_annotations_to_model_request() {
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
    host.seed_model_response_text("draft");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(44)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            max_tool_rounds: 0,
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
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let requests = host.model_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].model,
        etas_host::ModelName("local-qwen".to_owned())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_applies_agent_annotations_to_model_request() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.limits.Tokens;

tool Search() -> string {
  return "local";
}

@model("annotated-model", adapter = "mock-provider")
@tools([Search])
@limits([Tokens(31)])
@trace(VirtualStages([Logical, Search]))
agent Writer(input: string) -> string ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
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
            trace: TraceContext::root(TraceId(144)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            max_tool_rounds: 0,
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
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let requests = host.model_requests();
    let request = requests.first().expect("model request should be sent");
    assert_eq!(
        request.provider,
        Some(ModelProviderId("mock-provider".to_owned()))
    );
    assert_eq!(
        request.model,
        etas_host::ModelName("annotated-model".to_owned())
    );
    assert_eq!(request.tools.len(), 1);
    assert_eq!(request.options.max_output_tokens, Some(31));
    assert_eq!(
        request.budget.limits().tokens,
        Some(TokenBudget { max_tokens: 31 })
    );
    assert!(
        request.options.metadata.is_empty(),
        "@trace must stay in interpreter trace metadata, not provider request metadata"
    );
    assert!(
        result.events.iter().any(|event| {
            matches!(
                event,
                WorkflowEvent::AgentTracePlan { trace, .. }
                    if trace == &vec!["VirtualStages([Logical, Search])".to_owned()]
            )
        }),
        "agent @trace annotation should produce an internal trace plan event: {:?}",
        result.events
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_keeps_locked_runtime_model_over_agent_model_annotation() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

@model(model = "fixture-model")
agent Writer(input: string) -> string ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
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
            trace: TraceContext::root(TraceId(45)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            model: etas_host::ModelName("runtime-model".to_owned()),
            model_locked: true,
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
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let requests = host.model_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].model,
        etas_host::ModelName("runtime-model".to_owned())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_applies_agent_limits_annotation_to_model_request() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.limits.Tokens;

@limits([Tokens(17)])
agent Writer(input: string) -> string ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  return Writer.run(input);
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
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let requests = host.model_requests();
    let request = requests.first().expect("model request should be sent");
    assert_eq!(request.options.max_output_tokens, Some(17));
    assert_eq!(
        request.budget.limits().tokens,
        Some(TokenBudget { max_tokens: 17 })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_applies_pipeline_stage_token_limit_to_model_request() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.limits.Tokens;

agent Writer(input: string) -> string ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> string {
  return input ~> Writer limit Tokens(23);
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
            vec![value::InterpValue::String("hello".to_owned())],
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let requests = host.model_requests();
    let request = requests.first().expect("model request should be sent");
    assert_eq!(request.options.max_output_tokens, Some(23));
    assert_eq!(
        request.budget.limits().tokens,
        Some(TokenBudget { max_tokens: 23 })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_decodes_typed_agent_record_output_from_json() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

type Draft = { title: string, score: i64 };

@model(model = "local-qwen")
agent Writer(input: string) -> Draft ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> Draft {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text(r#"{"title":"Hello","score":7}"#);
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(45)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            max_tool_rounds: 0,
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
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let requests = host.model_requests();
    let schema = requests
        .first()
        .and_then(|request| request.response_schema.as_ref())
        .expect("typed agent request should carry an output schema");
    match schema {
        HostSchema::Record(fields) => {
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0].name, "title");
            assert_eq!(fields[0].schema, HostSchema::String);
            assert_eq!(fields[1].name, "score");
            assert_eq!(fields[1].schema, HostSchema::Int);
        }
        other => panic!("expected record output schema, got {other:?}"),
    }
    let Some(value::InterpValue::Nominal { value, .. }) = result.value().cloned() else {
        panic!("typed agent output must preserve nominal runtime identity");
    };
    assert_eq!(
        *value,
        value::InterpValue::Record(
            vec![
                (
                    "title".to_owned(),
                    value::InterpValue::String("Hello".to_owned())
                ),
                ("score".to_owned(), value::InterpValue::i64(7)),
            ]
            .into()
        )
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_typed_agent_json_decode_context() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

type Draft = { title: string, score: i64 };

agent Writer(input: string) -> Draft ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> Draft {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("not json output");
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
            provider: Some(ModelProviderId("mock-provider".to_owned())),
            provider_capabilities: Some(full_model_capabilities()),
            model: ModelName("mock-model".to_owned()),
            max_tool_rounds: 0,
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
        .await
        .expect("execution lifecycle infrastructure");

    let diagnostic_text = result
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        diagnostic_text.contains("mock-provider/mock-model"),
        "{diagnostic_text}"
    );
    assert!(diagnostic_text.contains("Record"), "{diagnostic_text}");
    assert!(diagnostic_text.contains("text"), "{diagnostic_text}");
    assert!(
        diagnostic_text.contains("not json output"),
        "{diagnostic_text}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_repairs_invalid_typed_agent_json_output() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

type Draft = { title: string, score: i64 };

agent Writer(input: string) -> Draft ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> Draft {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("not json output");
    host.seed_model_response_text(r#"{"title":"Fixed","score":9}"#);
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(47)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider: Some(ModelProviderId("mock-provider".to_owned())),
            provider_capabilities: Some(full_model_capabilities()),
            model: ModelName("mock-model".to_owned()),
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
        .await
        .expect("execution lifecycle infrastructure");

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.model_call_count(), 2);
    let Some(value::InterpValue::Nominal { value, .. }) = result.value().cloned() else {
        panic!("repaired typed output must preserve nominal runtime identity");
    };
    assert_eq!(
        *value,
        value::InterpValue::Record(
            vec![
                (
                    "title".to_owned(),
                    value::InterpValue::String("Fixed".to_owned())
                ),
                ("score".to_owned(), value::InterpValue::i64(9)),
            ]
            .into()
        )
    );
    let requests = host.model_requests();
    let repair_request = requests.get(1).expect("repair model request");
    assert!(
        repair_request.messages.iter().any(|message| {
            message.role == ModelRole::User
                && message.content.iter().any(|content| {
                    matches!(
                        content,
                        ModelContent::Text(text)
                            if text.contains("previous response did not satisfy")
                                && text.contains("valid JSON")
                    )
                })
        }),
        "{repair_request:#?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_typed_agent_integer_out_of_range() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

@model(model = "local-qwen")
agent Writer(input: string) -> i8 ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> i8 {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("200");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(47)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            max_tool_rounds: 0,
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
        .await
        .expect("execution lifecycle infrastructure");

    assert_eq!(result.value().cloned(), None);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::InvalidArguments)
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_typed_agent_trusted_output_from_model_json() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;

@model(model = "local-qwen")
agent Writer(input: string) -> Trusted<string> ![] {
  return Prompt.new().user(Public(input));
}

flow main(input: string) -> Trusted<string> {
  return Writer.run(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text(r#""draft""#);
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Agentic", "infer")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(48)),
            budget: etas_host::ExecutionBudget::default(),
        },
        model_policy: api::ModelExecutionPolicy {
            provider_capabilities: Some(full_model_capabilities()),
            max_tool_rounds: 0,
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
        .await
        .expect("execution lifecycle infrastructure");

    assert_eq!(result.value().cloned(), None);
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::InvalidArguments)
    }));
}

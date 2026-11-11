use super::super::*;
use etas_host::{ModelMessage, ModelToolCall};

#[tokio::test(flavor = "current_thread")]
async fn generic_tool_invocation_preserves_context_and_full_transitive_summary() {
    let (checked, definitions) = super::compiler_definitions::prepared(
        r#"
module app.main;
import std.agent.{ToolCall, ToolResult};
import std.json.JsonValue;
import std.io.println;
tool echo(text: string) -> string ![] { println(text)?; return std.text.uppercase(text); }
tool fail() -> string ![Error<i32>] { return perform Error<i32>.raise(99); }
flow execute<C>(call: ToolCall<C>) -> ToolResult<C> { return std.tool.invoke<C>(call); }
@tools([echo, fail])
agent Runner() -> string ![Error<i32>] {
    let call = ToolCall<Runner.run> { id = "generic", tool = std.tool.reference(echo), args = JsonValue.Object({"text" => JsonValue.String("hi")}) };
    let result = execute(call);
    return match result.result { Ok(JsonValue.String(text)) => text, Err(error) => abort(error.message), _ => abort("wrong result"), };
}
flow main() -> string {
    return handle Runner.run() with { Error<i32>.raise(_) => { finish "error"; } };
}
"#,
        &["app.main.echo", "app.main.fail"],
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let mut options = RunOptions {
        code_definitions: Some(definitions),
        ..Default::default()
    };
    options
        .host_context
        .authority
        .grants
        .push(HostActionGrant::allow("Console", "stdout_write"));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::String("HI".into())));
    assert_eq!(host.stdout_text(), "hi\n");
    assert_eq!(host.console_call_count(), 1);
    assert!(host.model_requests().is_empty());
    let runner = checked
        .hir
        .items
        .iter()
        .find_map(|(item, declaration)| match declaration {
            etas_hir::HirItem::Agent(agent)
                if checked.hir.symbols.get(agent.symbol).unwrap().name == "Runner" =>
            {
                Some(item)
            }
            _ => None,
        })
        .unwrap();
    let summary = &checked.effects.item_effects[&runner];
    assert!(
        summary
            .escaping_effects
            .effects
            .iter()
            .any(|effect| matches!(effect, etas_effects::Effect::Error(_)))
    );
    assert!(summary.requested_actions.effects.iter().any(|effect| matches!(effect, etas_effects::Effect::Action(action) if action.tag == etas_effects::CONSOLE_TAG)));
    assert!(summary.action_trace.action_set().iter().any(|effect| matches!(effect, etas_effects::Effect::Action(action) if action.tag == etas_effects::CONSOLE_TAG)));
}

#[tokio::test(flavor = "current_thread")]
async fn default_infer_controller_uses_the_same_steps_tools_history_and_checkpoint_contract_as_source()
 {
    let (checked, definitions) = super::compiler_definitions::prepared(
        r#"
module app.main;
import std.agent.{StepDecision, ToolResult};
import std.agent.conversation;
import std.runtime.checkpoint;
tool uppercase(text: string) -> string ![] { checkpoint("native-tool"); return std.text.uppercase(text); }
@tools([uppercase])
agent Writer() -> string { return perform infer<string>(Prompt.new().data("hi")); }
flow main(manual: bool) -> string {
    if manual {
        return handle Writer.run() with {
            Agentic.infer<Writer.run, string>(prompt, schema) => {
                var history = conversation.from_prompt(prompt);
                for _ in [0, 3) limit Iterations(3) {
                    let step = std.agent.step(history, schema);
                    history = match conversation.append_step(history, step) { Ok(next) => next, Err(error) => abort(error.message), };
                    match step.decision {
                        StepDecision.Final(answer) => { resume answer; },
                        StepDecision.CallTools(calls) => {
                            var results: Array<ToolResult<Writer.run>> = [];
                            for call in calls limit Iterations(8) { results = results.push(std.tool.invoke(call)); }
                            history = match conversation.append_results(history, results) { Ok(next) => next, Err(error) => abort(error.message), };
                        },
                        StepDecision.Stopped(_) => abort("stopped"),
                    };
                }
                abort("limit");
            }
        };
    }
    return Writer.run();
}
"#,
        &["app.main.uppercase"],
    );
    let mut options = RunOptions {
        code_definitions: Some(definitions),
        ..Default::default()
    };
    for action in ["infer", "step"] {
        options
            .host_context
            .authority
            .grants
            .push(HostActionGrant::allow("Agentic", action));
    }
    options.model_policy.provider_capabilities = Some(full_model_capabilities());
    let services = availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
    ]);
    let mut histories = Vec::new();
    for manual in [false, true] {
        let host = FakeHost::new(services);
        host.seed_model_response_tool_call_with_args(
            "uppercase",
            HostValue::Record(vec![("text".into(), HostValue::String("hi".into()))]),
        );
        host.seed_model_response_text("HI");
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![InterpValue::Bool(manual)],
                &host,
                options.clone(),
            )
            .await
            .unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "manual={manual}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.value(), Some(&InterpValue::String("HI".into())));
        assert_eq!(host.model_call_count(), 2);
        assert!(host.tool_requests().is_empty());
        histories.push(
            host.model_requests()
                .into_iter()
                .map(|request| request.messages)
                .collect::<Vec<_>>(),
        );
        let checkpoint = crate::api::codec::checkpoint_from_json(
            &crate::api::codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0])
                .unwrap(),
            &checked,
        )
        .unwrap();
        let resumed_host = FakeHost::new(services);
        resumed_host.seed_model_response_text("HI");
        let resumed = Interpreter
            .resume_checkpoint(&checked, &checkpoint, &resumed_host, options.clone())
            .await
            .unwrap();
        assert!(
            resumed.diagnostics.is_empty(),
            "manual={manual}: {:?}",
            resumed.diagnostics
        );
        assert_eq!(resumed.value(), result.value());
        assert_eq!(resumed_host.model_call_count(), 1);
    }
    assert_eq!(histories[0], histories[1]);
}

#[tokio::test(flavor = "current_thread")]
async fn caller_constructed_native_tool_invocation_preserves_effects_and_checkpoint_completion() {
    let (checked, definitions) = super::compiler_definitions::prepared(
        r#"
module app.main;
import std.agent.ToolCall;
import std.json.JsonValue;
import std.runtime.checkpoint;
import std.io.println;
tool echo(text: string) -> string ![] {
    checkpoint("inside-tool");
    println(text)?;
    return std.text.uppercase(text);
}
@tools([echo])
agent Runner() -> string {
    let call = ToolCall<Runner.run> { id = "manual-1", tool = std.tool.reference(echo), args = JsonValue.Object({"text" => JsonValue.String("hi")}) };
    let first = std.tool.invoke(call);
    checkpoint("after-tool");
    let second = std.tool.invoke(call);
    if first != second { abort("completed invocation changed"); }
    return match second.result { Ok(JsonValue.String(text)) => text, Err(error) => abort(error.message), _ => abort("wrong output"), };
}
flow main() -> string { return Runner.run(); }
"#,
        &["app.main.echo"],
    );
    let mut options = RunOptions {
        code_definitions: Some(definitions),
        ..Default::default()
    };
    options
        .host_context
        .authority
        .grants
        .push(HostActionGrant::allow("Console", "stdout_write"));
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options.clone(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::String("HI".into())));
    assert_eq!(host.stdout_text(), "hi\n");
    assert_eq!(host.console_call_count(), 1);
    assert!(host.model_requests().is_empty());
    for (checkpoint, calls) in result.checkpoints.iter().zip([1, 0]) {
        let artifact =
            crate::api::codec::checkpoint_artifact_json(&[], "main", checkpoint).unwrap();
        let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
        let resumed_host = FakeHost::new(services);
        let resumed = Interpreter
            .resume_checkpoint(&checked, &checkpoint, &resumed_host, options.clone())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), result.value());
        assert_eq!(resumed_host.console_call_count(), calls);
    }
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(summary.requested_actions.effects.iter().any(|effect| matches!(effect, etas_effects::Effect::Action(action) if action.tag == etas_effects::CONSOLE_TAG)));
}

#[tokio::test(flavor = "current_thread")]
async fn caller_tool_records_are_checked_before_execution() {
    let (checked, definitions) = super::compiler_definitions::prepared(
        r#"
module app.main;
import std.agent.{ToolCall, ToolRef};
import std.json.JsonValue;
tool echo(text: string) -> string ![] { std.io.println(text)?; return text; }
tool other(text: string) -> string ![] { std.io.println(text)?; return text; }
@tools([echo])
agent Runner(mode: i32, expected: string) -> bool {
    let native = std.tool.reference(echo);
    let reference = match mode {
        0 => ToolRef { id = "unknown", revision = native.revision },
        1 => ToolRef { id = native.id, revision = "old-revision" },
        2 => std.tool.reference(other),
        _ => native,
    };
    let args = if mode == 3 { JsonValue.Object({"text" => JsonValue.Bool(true)}) }
        else { JsonValue.Object({"text" => JsonValue.String("must not execute")}) };
    let call = ToolCall<Runner.run> { id = "caller", tool = reference, args = args };
    return match std.tool.invoke(call).result { Err(error) => error.code == expected, Ok(_) => false };
}
flow main(mode: i32, expected: string) -> bool { return Runner.run(mode, expected); }
"#,
        &["app.main.echo", "app.main.other"],
    );
    let mut options = RunOptions {
        code_definitions: Some(definitions),
        ..Default::default()
    };
    options
        .host_context
        .authority
        .grants
        .push(HostActionGrant::allow("Console", "stdout_write"));
    for (mode, expected) in [
        (0, "tool_not_exposed"),
        (1, "stale_tool_revision"),
        (2, "tool_not_exposed"),
        (3, "invalid_arguments"),
    ] {
        let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![InterpValue::i32(mode), InterpValue::String(expected.into())],
                &host,
                options.clone(),
            )
            .await
            .unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "mode={mode}: {:?}",
            result.diagnostics
        );
        assert_eq!(
            result.value(),
            Some(&InterpValue::Bool(true)),
            "mode={mode}"
        );
        assert_eq!(host.console_call_count(), 0);
        assert!(host.tool_requests().is_empty());
        assert!(host.model_requests().is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn nested_agent_tool_context_restores_the_outer_context_after_checkpoint() {
    let (checked, definitions) = super::compiler_definitions::prepared(
        r#"
module app.main;
import std.agent.ToolCall;
import std.json.JsonValue;
import std.runtime.checkpoint;
tool outer() -> unit ![] { std.io.println("outer")?; }
tool inner() -> unit ![] { std.io.println("inner")?; }
@tools([inner])
agent Inner() -> unit {
    std.tool.invoke(ToolCall<Inner.run> { id = "inner", tool = std.tool.reference(inner), args = JsonValue.Object({}) });
    checkpoint("nested-context");
}
@tools([outer])
agent Outer() -> unit {
    std.tool.invoke(ToolCall<Outer.run> { id = "before", tool = std.tool.reference(outer), args = JsonValue.Object({}) });
    Inner.run();
    std.tool.invoke(ToolCall<Outer.run> { id = "after", tool = std.tool.reference(outer), args = JsonValue.Object({}) });
}
flow main() -> unit { Outer.run(); }
"#,
        &["app.main.outer", "app.main.inner"],
    );
    let mut options = RunOptions {
        code_definitions: Some(definitions),
        ..Default::default()
    };
    options
        .host_context
        .authority
        .grants
        .push(HostActionGrant::allow("Console", "stdout_write"));
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options.clone(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.stdout_text(), "outer\ninner\nouter\n");
    let artifact =
        crate::api::codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let host = FakeHost::new(services);
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, options)
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(host.stdout_text(), "outer\n");
    assert_eq!(resumed.value(), result.value());
}

#[tokio::test(flavor = "current_thread")]
async fn native_tool_typed_error_escapes_invoke_instead_of_becoming_feedback() {
    let (checked, definitions) = super::compiler_definitions::prepared(
        r#"
module app.main;
import std.agent.ToolCall;
import std.json.JsonValue;
tool fail() -> i32 ![Error<i32>] { return perform Error<i32>.raise(17); }
@tools([fail])
agent Runner() -> i32 ![Error<i32>] {
    let call = ToolCall<Runner.run> { id = "failing", tool = std.tool.reference(fail), args = JsonValue.Object({}) };
    let ignored = std.tool.invoke(call);
    return 0;
}
flow main() -> i32 {
    return handle { return Runner.run(); } with { Error<i32>.raise(error) => { finish error; } };
}
"#,
        &["app.main.fail"],
    );
    let host = FakeHost::new(HostServiceAvailability::default());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions {
                code_definitions: Some(definitions),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(17)));
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_step_controller_preserves_native_references_provider_data_and_checkpoint_context()
{
    let (checked, definitions) = super::compiler_definitions::prepared(
        r#"
module app.main;
import std.agent.{StepDecision, StepResult, ToolResult};
import std.agent.conversation;
import std.json.JsonValue;
import std.runtime.checkpoint;
tool uppercase(text: string) -> string ![] { return std.text.uppercase(text); }
flow one<C, O>(history: Conversation, schema: Schema<O>) -> StepResult<C, O> {
    return std.agent.step<C, O>(history, schema);
}
@tools([uppercase])
agent Writer() -> string { return perform infer<string>(Prompt.new().data("answer")); }
flow main() -> string {
    return handle Writer.run() with {
        Agentic.infer<Writer.run, string>(prompt, schema) => {
            var history = conversation.from_prompt(prompt);
            let first = one<Writer.run, string>(history, schema);
            let calls = match first.decision { StepDecision.CallTools(calls) => calls, _ => abort("expected tool request"), };
            if calls[0].tool != std.tool.reference(uppercase) { abort("tool identity mismatch"); }
            history = match conversation.append_step(history, first) { Ok(next) => next, Err(error) => abort(error.message), };
            let invoked = std.tool.invoke(calls[0]);
            match invoked.result { Ok(JsonValue.String(text)) => { if text != "HI" { abort("wrong native result"); } }, _ => abort("native tool failed"), };
            history = match conversation.append_results(history, [invoked]) { Ok(next) => next, Err(error) => abort(error.message), };
            checkpoint("controller");
            let second = one<Writer.run, string>(history, schema);
            match second.decision { StepDecision.Final(answer) => { resume answer; }, _ => abort("expected final answer"), };
        }
    };
}
"#,
        &["app.main.uppercase"],
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
    ]));
    let metadata = vec![(
        "openai.v1".into(),
        HostValue::Json(etas_host::HostJsonValue::Object(vec![(
            "reasoning_content".into(),
            etas_host::HostJsonValue::String("retained continuation".into()),
        )])),
    )];
    host.seed_model_output(
        etas_host::ModelOutput {
            message: ModelMessage {
                provider_data: metadata.clone(),
                role: ModelRole::Assistant,
                content: vec![],
                tool_call_id: None,
                tool_calls: vec![ModelToolCall {
                    id: "call-1".into(),
                    tool: "uppercase".into(),
                    args: HostValue::Record(vec![("text".into(), HostValue::String("hi".into()))]),
                }],
            },
            termination: etas_host::ModelTermination::Complete,
            provider_request_id: Some("first".into()),
        },
        None,
    );
    host.seed_model_response_text("complete");
    let mut options = RunOptions {
        code_definitions: Some(definitions),
        ..Default::default()
    };
    options
        .host_context
        .authority
        .grants
        .push(HostActionGrant::allow("Agentic", "step"));
    options.model_policy.provider_capabilities = Some(full_model_capabilities());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options.clone(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::String("complete".into()))
    );
    assert_eq!(host.model_call_count(), 2);
    assert!(host.tool_requests().is_empty());
    assert_eq!(
        result.checkpoints[0]
            .completed_host_boundaries
            .completed
            .iter()
            .filter(|boundary| boundary.kind == "tool")
            .count(),
        1
    );
    let requests = host.model_requests();
    let assistant = &requests[1].messages[1];
    assert_eq!(assistant.provider_data, metadata);
    assert_eq!(assistant.tool_calls.len(), 1);
    assert_eq!(
        requests[1].messages[2].tool_call_id.as_deref(),
        Some("call-1")
    );
    let artifact =
        crate::api::codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed_host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    resumed_host.seed_model_response_text("complete");
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &resumed_host, options)
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), result.value());
    assert_eq!(resumed_host.model_call_count(), 1);
    assert_eq!(
        resumed_host.model_requests()[0].messages[1].provider_data,
        metadata
    );
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_model_profile_does_not_expand_an_agents_declared_tool_surface() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.{StepDecision};
import std.agent.conversation;
agent Writer() -> string { return perform infer<string>(Prompt.new().data("answer")); }
flow main(manual: bool) -> string {
    if manual {
        return handle Writer.run() with {
            Agentic.infer<Writer.run, string>(prompt, schema) => {
                let step = std.agent.step(conversation.from_prompt(prompt), schema);
                match step.decision {
                    StepDecision.Final(answer) => { resume answer; },
                    _ => abort("expected final"),
                };
            }
        };
    }
    return Writer.run();
}
"#,
    );
    for manual in [false, true] {
        let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
        host.seed_model_response_text("done");
        let mut options = RunOptions::default();
        options.model_policy.provider_capabilities = Some(full_model_capabilities());
        options.model_policy.tools.push(ToolSchema {
            tool: ToolRef::external("not_exposed", "external.not_exposed"),
            input: HostSchema::Unit,
            output: None,
        });
        options.model_policy.tool_choice = etas_host::ModelToolChoice::RequiredAny;
        for action in ["infer", "step"] {
            options
                .host_context
                .authority
                .grants
                .push(HostActionGrant::allow("Agentic", action));
        }
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![InterpValue::Bool(manual)],
                &host,
                options,
            )
            .await
            .unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "manual={manual}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.value(), Some(&InterpValue::String("done".into())));
        let requests = host.model_requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].tools.is_empty());
        assert_eq!(requests[0].tool_choice, etas_host::ModelToolChoice::Auto);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn single_step_returns_final_or_stopped_without_tool_execution_or_repair() {
    let source = r#"
module app.main;
import std.agent.{StepDecision, StopReason};
import std.agent.conversation;
agent Writer() -> bool { return perform infer<bool>(Prompt.new().data("answer")); }
flow main(expected: string) -> bool {
    return handle Writer.run() with {
        Agentic.infer<Writer.run, bool>(prompt, schema) => {
            let history = conversation.from_prompt(prompt);
            let result = std.agent.step(history, schema);
            let kept_usage = match result.usage {
                Some(usage) => usage.input_tokens == Some(2) && usage.output_tokens == None && match usage.cost {
                    Some(cost) => cost.micros == 17 && cost.currency == "USD",
                    None => false,
                },
                None => false,
            };
            if !kept_usage { abort("lost partial usage or cost"); }
            if result.turn.request_id != Some("provider-1") { abort("lost provider request id"); }
            let answer = match result.decision {
                StepDecision.Final(value) => if value { "final" } else { "false" },
                StepDecision.CallTools(_) => "tools",
                StepDecision.Stopped(StopReason.Truncated) => "truncated",
                StepDecision.Stopped(StopReason.Refused(_)) => "refused",
                StepDecision.Stopped(StopReason.InvalidOutput(_)) => "invalid_output",
                StepDecision.Stopped(StopReason.InvalidToolCall(_)) => "invalid_tool",
            };
            resume answer == expected;
        }
    };
}
"#;
    let (checked, definitions) = super::compiler_definitions::prepared(source, &[]);
    for (termination, text, tool, expected) in [
        (etas_host::ModelTermination::Complete, "true", None, "final"),
        (
            etas_host::ModelTermination::Truncated,
            "tru",
            None,
            "truncated",
        ),
        (
            etas_host::ModelTermination::Refused("declined".into()),
            "",
            None,
            "refused",
        ),
        (
            etas_host::ModelTermination::Complete,
            "not json",
            None,
            "invalid_output",
        ),
        (
            etas_host::ModelTermination::Complete,
            "",
            Some("foreign_tool"),
            "invalid_tool",
        ),
        (
            etas_host::ModelTermination::InvalidToolCall("invalid JSON arguments".into()),
            "",
            None,
            "invalid_tool",
        ),
    ] {
        let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
        host.seed_model_output(
            etas_host::ModelOutput {
                message: ModelMessage {
                    provider_data: Vec::new(),
                    role: ModelRole::Assistant,
                    content: vec![ModelContent::Text(text.into())],
                    tool_call_id: None,
                    tool_calls: tool
                        .into_iter()
                        .map(|tool| ModelToolCall {
                            id: "call-1".into(),
                            tool: tool.into(),
                            args: HostValue::Record(Vec::new()),
                        })
                        .collect(),
                },
                termination,
                provider_request_id: Some("provider-1".into()),
            },
            Some(etas_host::ModelUsage {
                input_tokens: Some(2),
                output_tokens: None,
                cost: Some(etas_host::ModelCostUsage {
                    micros: 17,
                    currency: "USD".into(),
                }),
            }),
        );
        let mut options = RunOptions {
            code_definitions: Some(definitions.clone()),
            ..Default::default()
        };
        options
            .host_context
            .authority
            .grants
            .push(HostActionGrant::allow("Agentic", "step"));
        options.model_policy.provider_capabilities = Some(full_model_capabilities());
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![InterpValue::String(expected.into())],
                &host,
                options,
            )
            .await
            .unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "{expected}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(host.model_call_count(), 1, "{expected}");
        assert!(host.tool_requests().is_empty());
        assert_eq!(
            host.model_requests()[0].options.attempt_policy,
            etas_host::ModelAttemptPolicy::Single
        );
        assert_eq!(
            host.model_requests()[0].action,
            etas_host::ModelAction::Step
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_assembly_is_sessionless_pure_correlated_and_checkpointable() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.{AssistantTurn, MessageContent, ToolRef, ToolCall, ToolResult, StepDecision, StepResult};
import std.agent.conversation;
import std.json.JsonValue;
import std.runtime.checkpoint;
type Context;
flow main() -> std.agent.session.Conversation {
    let original = conversation.from_prompt(Prompt.new().system(Trusted("rules")).data("untrusted input"));
    if original.session != None { return abort("fabricated session"); }
    let tool = ToolRef { id = "simulation-only", revision = "not-registered" };
    let first = ToolCall<Context> { id = "first", tool = tool, args = JsonValue.Null };
    let second = ToolCall<Context> { id = "second", tool = tool, args = JsonValue.Bool(true) };
    let turn = AssistantTurn { request_id = None, content = [MessageContent.Text("use two tools")], provider_data = {} };
    let step = StepResult<Context, string> { turn = turn, decision = StepDecision.CallTools([first, second]), usage = None };
    let pending = match conversation.append_step(original, step) { Ok(value) => value, Err(error) => abort(error.message), };
    checkpoint("pending-tools");
    let first_result = ToolResult<Context> { call_id = "first", result = Ok(JsonValue.String("one")) };
    let second_result = ToolResult<Context> { call_id = "second", result = Ok(JsonValue.String("two")) };
    let complete = match conversation.append_results(pending, [second_result, first_result]) { Ok(value) => value, Err(error) => abort(error.message), };
    if original == complete { return abort("lost protocol history"); }
    let final_step = StepResult<Context, string> { turn = turn, decision = StepDecision.Final("complete"), usage = None };
    let finished = match conversation.append_step(complete, final_step) { Ok(value) => value, Err(error) => abort(error.message), };
    match conversation.append_results(finished, [first_result, second_result]) {
        Ok(_) => abort("results after Final must not be accepted"),
        Err(error) => { if error.code != "invalid_protocol" { return abort(error.message); } },
    };
    return finished;
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let value = result.value().unwrap();
    let encoded = crate::api::codec::value_json(value).unwrap();
    assert_eq!(
        crate::api::codec::value_from_json(&encoded).unwrap(),
        *value
    );
    let snapshot = crate::orchestration::ValueSnapshot::capture(value).unwrap();
    assert_eq!(snapshot.restore().unwrap(), *value);
    let artifact =
        crate::api::codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(value));
    let InterpValue::Conversation(conversation) = value else {
        panic!("conversation");
    };
    let (prompt, turns) =
        crate::value::conversation::protocol::parts(conversation.protocol.as_deref().unwrap())
            .unwrap();
    assert_eq!(prompt.len(), 2);
    assert_eq!(prompt[0].trust, Some(etas_types::TrustWrapper::Trusted));
    assert_eq!(prompt[1].role, crate::value::PromptRole::Data);
    let events = turns.iter().collect::<Vec<_>>();
    let InterpValue::Array(results) = events[1] else {
        panic!("tool results");
    };
    let ids = results
        .borrow()
        .iter()
        .map(|result| {
            crate::value::conversation::protocol::field(result, "call_id")
                .unwrap()
                .clone()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            InterpValue::String("first".into()),
            InterpValue::String("second".into())
        ]
    );
    assert!(host.model_requests().is_empty());
    assert!(host.tool_requests().is_empty());
    assert_eq!(host.session_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn agent_step_protocol_is_transparent_data_not_execution_evidence() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.{AssistantTurn, MessageContent, ToolRef, ToolCall, ToolResult, StepDecision, StepResult, StepDiagnostic, StopReason, Usage};
import std.json.JsonValue;
type Context;

flow main() -> bool {
    let tool: std.compiler.ToolRef = ToolRef { id = "manual", revision = "manual-revision" };
    let call = ToolCall<Context> { id = "call-1", tool = tool, args = JsonValue.Null };
    let turn = AssistantTurn { request_id = None, content = [MessageContent.Text("call a tool")], provider_data = {} };
    let usage = Usage { input_tokens = Some(3), output_tokens = None, cost = Some({ micros = 17, currency = "USD" }) };
    let step = StepResult<Context, string> { turn = turn, decision = StepDecision.CallTools([call]), usage = Some(usage) };
    let result = ToolResult<Context> { call_id = "call-1", result = Ok(JsonValue.String("simulated")) };
    let diagnostic = StepDiagnostic { code = "invalid_arguments", message = "bad input", path = Some("value") };
    let stopped: StepDecision<Context, string> = StepDecision.Stopped(StopReason.InvalidToolCall(diagnostic));
    let requested = match step.decision {
        StepDecision.Final(_) => false,
        StepDecision.CallTools(calls) => calls[0].tool.id == "manual" && calls[0].tool.revision == "manual-revision",
        StepDecision.Stopped(_) => false,
    };
    let rejected = match stopped {
        StepDecision.Stopped(StopReason.InvalidToolCall(error)) => error.code == "invalid_arguments",
        _ => false,
    };
    let reported = match usage.cost { Some(cost) => cost.micros == 17 && cost.currency == "USD", None => false };
    return requested && rejected && reported && result.call_id == "call-1";
}
"#,
    );
    let host = FakeHost::new(HostServiceAvailability::default());
    let budget = etas_host::ExecutionBudget::start(Budget {
        tokens: Some(TokenBudget { max_tokens: 100 }),
        cost: Some(etas_host::CostBudget {
            max_micros: 100,
            currency: "USD".into(),
        }),
        ..Default::default()
    });
    let mut options = RunOptions::default();
    options.host_context.budget = budget.clone();
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert!(host.model_requests().is_empty());
    assert!(host.tool_requests().is_empty());
    let progress = budget.snapshot().unwrap();
    assert_eq!(progress.consumed_tokens, 0);
    assert_eq!(progress.reserved_tokens, 0);
    assert_eq!(progress.consumed_cost_micros, 0);
    assert_eq!(progress.reserved_cost_micros, 0);
}

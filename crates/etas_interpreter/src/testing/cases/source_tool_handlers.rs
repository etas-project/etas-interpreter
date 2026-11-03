use super::super::*;

fn options() -> RunOptions {
    let mut options = RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(HostActionGrant::allow("Agentic", "infer"));
    options.model_policy.provider_capabilities = Some(full_model_capabilities());
    options.execution_limits.max_steps = std::num::NonZeroU64::new(2000);
    options
}

fn project(body: &str, arm: &str) -> etas_frontend::CheckedProgram {
    checked_project(&format!(
        r#"
module app.main;
import std.runtime.checkpoint;
import std.io.println;
effect Evidence {{ action read() -> string; }}
tool lookup() -> string ![Evidence.read] {{ {body} }}
@tools([lookup])
agent Reader() -> string ![Evidence.read] {{ return perform infer(Prompt.new()); }}
flow main() -> string ![Agentic] {{
    return Reader.run() with {{ Evidence.read() => {{ {arm} }} }};
}}
"#
    ))
}

fn assert_tool_result(host: &FakeHost, expected: &str) {
    let requests = host.model_requests();
    assert_eq!(requests.len(), 2);
    let results = requests[1]
        .messages
        .iter()
        .filter(|message| message.role == etas_host::ModelRole::Tool)
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_call_id.as_deref(), Some("call-1"));
    assert_eq!(
        results[0].content,
        vec![ModelContent::Value(HostValue::String(expected.into()))]
    );
}

fn source_tool_node(value: &mut serde_json::Value) -> Option<&mut serde_json::Value> {
    if value.get("kind").and_then(serde_json::Value::as_str) == Some("source_tool_return") {
        return Some(value);
    }
    match value {
        serde_json::Value::Object(fields) => fields.values_mut().find_map(source_tool_node),
        serde_json::Value::Array(values) => values.iter_mut().find_map(source_tool_node),
        _ => None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn source_tool_repeated_effects_keep_one_tool_result_and_the_model_loop() {
    let checked = project(
        "let a = perform Evidence.read(); let b = perform Evidence.read(); return a + b;",
        "resume \"part\";",
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_tool_call("lookup");
    host.seed_model_response_text("finished");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::String("finished".into()))
    );
    assert_tool_result(&host, "partpart");
}

#[tokio::test(flavor = "current_thread")]
async fn source_tool_outer_finish_discards_the_tool_and_pending_model_round() {
    let checked = project("return perform Evidence.read();", "finish \"stopped\";");
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_tool_call("lookup");
    host.seed_model_response_text("must not run");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::String("stopped".into())));
    assert_eq!(host.model_call_count(), 1);
    assert!(host.tool_requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn source_tool_outer_handler_checkpoint_preserves_and_validates_the_tool_continuation() {
    let checked = project(
        "return perform Evidence.read();",
        "println(\"handling\")?; checkpoint(\"handler\"); resume \"local evidence\";",
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
        HostRequirementKind::Console,
    ]));
    host.seed_model_response_tool_call("lookup");
    host.seed_model_response_text("finished");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_tool_result(&host, "local evidence");
    assert_eq!(result.checkpoints.len(), 1);
    assert_eq!(host.stdout_text(), "handling\n");
    let artifact =
        api::codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    assert!(artifact.to_string().contains("source_tool_return"));
    let bytes = api::codec::checkpoint_file_to_bytes(artifact, Default::default()).unwrap();
    let decoded = api::codec::checkpoint_file_from_bytes(&bytes, Default::default()).unwrap();
    let restored = api::codec::checkpoint_from_json(&decoded, &checked).unwrap();
    let mut bad_binding = (*decoded).clone();
    source_tool_node(&mut bad_binding).unwrap()["source_tool"]["binding"]["item"] =
        serde_json::json!(u32::MAX);
    assert!(api::codec::checkpoint_from_json(&bad_binding, &checked).is_err());
    let mut bad_nested = (*decoded).clone();
    source_tool_node(&mut bad_nested).unwrap()["source_tool"]["model_loop"]["outer_continuation"] =
        serde_json::json!({"kind": "source_tool_return"});
    let error = api::codec::checkpoint_from_json(&bad_nested, &checked).unwrap_err();
    assert!(
        error
            .message()
            .contains("unexpected nested execution continuation"),
        "{error}"
    );
    let resumed_host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    resumed_host.seed_model_response_text("resumed");
    let resumed = Interpreter
        .resume_checkpoint(&checked, &restored, &resumed_host, options())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        resumed.value(),
        Some(&InterpValue::String("resumed".into()))
    );
    assert_eq!(resumed_host.model_call_count(), 1);
    assert!(resumed_host.stdout_text().is_empty());
    let requests = resumed_host.model_requests();
    assert!(
        requests[0]
            .messages
            .iter()
            .any(|message| message.role == etas_host::ModelRole::Tool
                && message.content
                    == vec![ModelContent::Value(HostValue::String(
                        "local evidence".into()
                    ))])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn source_tool_error_can_finish_an_outer_handle_without_another_model_round() {
    let checked = checked_project(
        r#"
module app.main;
tool lookup() -> string ![Error<IndexError>] { let values: Array<string> = []; return values[0]; }
@tools([lookup])
agent Reader() -> string ![Error<IndexError>] { return perform infer(Prompt.new()); }
flow main() -> string ![Agentic] {
    return Reader.run() with { Error<IndexError>.raise(_) => { finish "empty"; } };
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_tool_call("lookup");
    host.seed_model_response_text("must not run");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::String("empty".into())));
    assert_eq!(host.model_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn source_tool_forwards_to_outer_handler_and_reenters_its_inner_handler() {
    let checked = checked_project(
        r#"
module app.main;
effect Evidence { action read() -> string; }
effect Suffix { action read() -> string; }
tool lookup() -> string ![Evidence.read] {
    return handle {
        let value = perform Evidence.read();
        let suffix = perform Suffix.read();
        value + suffix
    } with { Suffix.read() => { resume ":inner"; } };
}
@tools([lookup])
agent Reader() -> string ![Evidence.read] { return perform infer(Prompt.new()); }
flow main() -> string ![Agentic] {
    return Reader.run() with { Evidence.read() => { resume "outer"; } };
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_tool_call("lookup");
    host.seed_model_response_text("finished");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::String("finished".into()))
    );
    assert_tool_result(&host, "outer:inner");
}

#[tokio::test(flavor = "current_thread")]
async fn source_tool_effect_resumes_through_an_outer_handler_into_the_model_loop() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
effect Evidence { action read() -> string; }
tool lookup() -> string ![Evidence.read] { return perform Evidence.read(); }
@tools([lookup])
agent Reader(question: string) -> string ![Evidence.read] {
    return perform infer(Prompt.new().user(Public(question)));
}
flow main() -> i32 ![Agentic, Error<IOError>] {
    let evidence: ![Evidence => [] for string] = handler {
        Evidence.read() => { resume "local evidence"; }
    };
    let answer = Reader.run("read evidence") with evidence;
    println(answer);
    return 0;
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Console,
    ]));
    host.seed_model_response_tool_call("lookup");
    host.seed_model_response_text("finished");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.stdout_text(), "finished\n");
    assert_eq!(
        result.value(),
        Some(&InterpValue::Number(value::NumericValue::i32(0)))
    );
    assert_eq!(host.model_call_count(), 2);
    assert!(host.tool_requests().is_empty());
    let requests = host.model_requests();
    let results = requests[1]
        .messages
        .iter()
        .filter(|message| message.role == etas_host::ModelRole::Tool)
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_call_id.as_deref(), Some("call-1"));
    assert_eq!(
        results[0].content,
        vec![ModelContent::Value(HostValue::String(
            "local evidence".into()
        ))]
    );
}

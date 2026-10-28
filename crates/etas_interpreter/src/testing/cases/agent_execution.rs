use super::super::*;

fn model_options() -> RunOptions {
    let mut options = RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(HostActionGrant::allow("Agentic", "infer"));
    options.model_policy.provider_capabilities = Some(full_model_capabilities());
    options
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_agent_default_and_auxiliary_methods_use_normal_flow_execution() {
    let checked = checked_project(
        r#"
module app.main;
agent Worker;
impl agent Worker {
    flow run(value: i32) -> i32 { return value + 1; }
    flow explain(value: i32) -> i32 { return run(value) + 1; }
}
flow main() -> i32 { return Worker(19) + Worker.explain(20); }
"#,
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
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::Number(value::NumericValue::i32(42)))
    );
    assert_eq!(host.model_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn auxiliary_agent_method_inference_has_its_own_action_identity() {
    let checked = checked_project(
        r#"
module app.main;
agent Worker() -> string { return "unused default"; }
impl agent Worker {
    flow explain() -> string { return perform infer<string>(Prompt.new()); }
}

flow main() -> string {
    return Worker.explain() with {
        Agentic.infer<Worker.explain, string>(_, _) => resume "checked auxiliary";
    };
}
"#,
    );
    let fact = checked.types.agent_inferences.values().next().unwrap();
    let method = &checked.types.agent_methods[&fact.method];
    assert_eq!(method.name, "explain");
    assert_eq!(method.body, fact.owner);
    assert_ne!(fact.agent, fact.owner);
    let host = FakeHost::new(HostServiceAvailability::default());
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
    assert_eq!(
        result.value(),
        Some(&InterpValue::String("checked auxiliary".into()))
    );
    assert_eq!(host.model_call_count(), 0);
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert_eq!(summary.requested_actions.effects.iter().count(), 1);
    assert!(summary.escaping_effects.effects.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn agent_binding_selects_generic_auxiliary_method_without_losing_generic_facts() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
agent Worker;
impl agent Worker { flow echo<T>(value: T) -> T { checkpoint("generic-method"); return value; } }
flow call(worker: Worker) -> i32 { return worker.echo<i32>(42); }
flow main() -> i32 { return call(Worker); }
"#,
    );
    assert_eq!(checked.types.agent_method_calls.len(), 1);
    assert!(
        checked
            .types
            .generic_instantiations
            .values()
            .any(|fact| fact.type_bindings.iter().any(|(name, _)| name == "T"))
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
    assert_eq!(
        result.value(),
        Some(&InterpValue::Number(value::NumericValue::i32(42)))
    );
    assert_eq!(result.checkpoints.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn pure_agent_returns_declared_value_without_model_service() {
    let checked = checked_project(
        r#"
module app.main;
agent Increment(value: i32) -> i32 { return value + 1; }
flow main() -> i32 { return Increment.run(41); }
"#,
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
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::Number(value::NumericValue::i32(42)))
    );
    assert_eq!(host.model_call_count(), 0);
    assert!(
        checked.effects.item_effects[&checked.entry.unwrap()]
            .requested_actions
            .effects
            .is_empty()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn generic_agent_default_and_run_entry_preserve_frame_bindings_at_checkpoint() {
    use crate::orchestration::{ContinuationSnapshot, MachineFrameSnapshot};
    let definition = "agent Echo; impl agent Echo { flow run<T>(value: T) -> T { checkpoint(\"generic-entry\"); return value; } }";
    for invocation in ["Echo<i32>(42)", "Echo.run<i32>(42)"] {
        let source = format!(
            "module app.main; import std.runtime.checkpoint; {definition} flow main() -> i32 {{ return {invocation}; }}"
        );
        let checked = checked_project(&source);
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
        assert_eq!(
            result.value(),
            Some(&InterpValue::Number(value::NumericValue::i32(42)))
        );
        let checkpoint = result.checkpoints.first().expect("generic body checkpoint");
        assert!(checkpoint.machine.frames.iter().any(|frame| matches!(frame,
                MachineFrameSnapshot::Block { continuation: ContinuationSnapshot::ContinueBlock { frame, .. } }
                | MachineFrameSnapshot::Continuation { continuation: ContinuationSnapshot::ContinueBlock { frame, .. } }
                    if frame.type_bindings.iter().any(|(name, ty)| name == "T" && matches!(checked.type_store.get(*ty), Some(etas_types::Type::Primitive(etas_types::PrimitiveType::I32))))
        )), "generic entry must keep its binding in the callee frame: {definition}, {invocation}: {:?}", checkpoint.machine);
        let artifact =
            crate::api::codec::checkpoint_artifact_json(&[], "main", checkpoint).unwrap();
        let restored = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
        let resumed = Interpreter
            .resume_checkpoint(&checked, &restored, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), result.value());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_return_is_data_not_an_implicit_model_request() {
    let checked = checked_project(
        r#"
module app.main;
agent PromptBuilder() -> Prompt { return Prompt.new().user(Public("data")); }
flow main() -> Prompt { return PromptBuilder(); }
"#,
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
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(matches!(result.value(), Some(InterpValue::Prompt(_))));
    assert_eq!(host.model_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_inference_runs_twice_then_executes_agent_postprocessing() {
    let checked = checked_project(
        r#"
module app.main;
agent Writer() -> string {
    let first = perform infer<string>(Prompt.new().user(Public("first")));
    let second = perform infer<string>(Prompt.new().user(Public(first)));
    return std.text.uppercase(second);
}
flow main() -> string { return Writer(); }
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("draft");
    host.seed_model_response_text("draft");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            model_options(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::String("DRAFT".into())));
    assert_eq!(host.model_call_count(), 2);
    assert_eq!(checked.types.agent_inferences.len(), 2);
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(summary.escaping_effects.effects.is_empty());
    assert_eq!(summary.requested_actions.effects.iter().count(), 1);
    assert_eq!(summary.default_actions.effects.iter().count(), 1);
    let etas_effects::ActionTraceDomain::Seq(events) = &summary.action_trace else {
        panic!(
            "both explicit inference sites must remain in the trace: {:?}",
            summary.action_trace
        );
    };
    assert_eq!(events.len(), 2);
    for event in events {
        assert!(matches!(event, etas_effects::ActionTraceDomain::Event(_)));
    }
    let requests = host.model_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].messages[0].content,
        vec![ModelContent::Text("draft".into())]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn inference_output_type_is_inferred_from_ordinary_expected_type() {
    let checked = checked_project(
        r#"
module app.main;
agent Counter() -> i32 {
    let value: i32 = perform infer(Prompt.new().user(Public("number")));
    return value + 1;
}
flow main() -> i32 { return Counter(); }
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("7");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            model_options(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::Number(value::NumericValue::i32(8)))
    );
    assert_eq!(host.model_call_count(), 1);
}

#[test]
fn inference_scope_and_argument_kinds_are_checked_not_guessed() {
    for source in [
        "module app.main; flow main() -> string { return perform infer<string>(Prompt.new()); }",
        "module app.main; agent A() -> string { return perform infer<string>(42); } flow main() -> string { return A(); }",
        "module app.main; agent A() -> string { return perform infer<string>(Prompt.new(), Prompt.new()); } flow main() -> string { return A(); }",
        "module app.main; agent A() -> string { return perform infer<![]>(Prompt.new()); } flow main() -> string { return A(); }",
        "module app.main; agent A() -> string { return Prompt.new(); } flow main() -> string { return A(); }",
    ] {
        let output =
            etas_frontend::Frontend.check_project(etas_frontend::ProjectInput::single_source(
                etas_frontend::SourceInput::anonymous(source),
            ));
        assert!(
            output.checked.is_none(),
            "invalid inference accepted: {source}"
        );
        assert!(!output.diagnostics.is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn inference_obeys_scoped_action_handler_without_model_authority() {
    let checked = checked_project(
        r#"
module app.main;
agent Writer() -> string {
    return handle {
        return perform infer<string>(Prompt.new().user(Public("input")));
    } with {
        Agentic.infer<_, string>(_, _) => { resume "mocked"; }
    };
}
flow main() -> string { return Writer(); }
"#,
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
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::String("mocked".into())));
    assert_eq!(host.model_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn caller_handler_matches_canonical_inference_action_across_modules() {
    let checked = super::super::project::checked_project_sources(vec![
        (
            etas_core::SourceId(0),
            "src/app/main.es",
            r#"
module app.main;
import app.writer.Writer;
flow main() -> string {
    return Writer() with {
        Agentic.infer<_, string>(_, _) => { resume "cross-module"; }
    };
}
"#,
        ),
        (
            etas_core::SourceId(1),
            "src/app/writer.es",
            r#"
module app.writer;
public agent Writer() -> string { return perform infer<string>(Prompt.new()); }
"#,
        ),
    ]);
    let host = FakeHost::new(HostServiceAvailability::default());
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
    assert_eq!(
        result.value(),
        Some(&InterpValue::String("cross-module".into()))
    );
    assert_eq!(host.model_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn handler_with_different_output_selector_does_not_intercept_inference() {
    let checked = checked_project(
        r#"
module app.main;
agent Writer() -> string {
    return perform infer<string>(Prompt.new()) with {
        Agentic.infer<_, i32>(_, _) => { resume 42; }
    };
}
flow main() -> string { return Writer(); }
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Agentic]));
    host.seed_model_response_text("real-model-result");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            model_options(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::String("real-model-result".into()))
    );
    assert_eq!(host.model_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_in_prompt_operand_preserves_explicit_inference_and_caller_return() {
    for definition in [
        "agent Writer() -> string { return perform infer<string>(build_prompt()); } flow main() -> string { return Writer(); }",
        "agent Writer; impl agent Writer { flow run() -> string { return perform infer<string>(build_prompt()); } } flow main() -> string { return Writer(); }",
        "agent Writer; impl agent Writer { flow explain() -> string { return perform infer<string>(build_prompt()); } } flow main() -> string { return Writer.explain(); }",
    ] {
        let source = format!(
            r#"
module app.main;
import std.runtime.checkpoint;
flow build_prompt() -> Prompt {{
    checkpoint("before-prompt");
    return Prompt.new().user(Public("input"));
}}
{definition}
"#
        );
        let checked = checked_project(&source);
        let services = [
            HostRequirementKind::Checkpoint,
            HostRequirementKind::Agentic,
        ];
        let host = FakeHost::new(availability(&services));
        host.seed_model_response_text("initial");
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![],
                &host,
                model_options(),
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::String("initial".into())));
        assert_eq!(host.model_call_count(), 1);
        let checkpoint = result
            .checkpoints
            .first()
            .expect("prompt construction checkpoint");
        let artifact = crate::api::codec::checkpoint_artifact_json(&[], "main", checkpoint);
        let artifact = artifact.expect("checkpoint encode");
        let restored = crate::api::codec::checkpoint_from_json(&artifact, &checked)
            .expect("checkpoint must retain the inference continuation");
        let resumed_host = FakeHost::new(availability(&services));
        resumed_host.seed_model_response_text("resumed");
        let resumed = Interpreter
            .resume_checkpoint(&checked, &restored, &resumed_host, model_options())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(
            resumed.value(),
            Some(&InterpValue::String("resumed".into()))
        );
        assert_eq!(resumed_host.model_call_count(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn default_inference_preserves_an_unrelated_scoped_handler() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
effect Gate { action request() -> string; }
agent Writer() -> string {
    return handle {
        let first = perform infer<string>(Prompt.new());
        checkpoint("after-inference");
        let second = perform Gate.request();
        return first + second;
    } with { Gate.request() => resume "handled"; };
}
flow main() -> string { return Writer(); }
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
    ]));
    host.seed_model_response_text("model-");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            model_options(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::String("model-handled".into()))
    );
    assert_eq!(host.model_call_count(), 1);
    let checkpoint = result
        .checkpoints
        .first()
        .expect("checkpoint inside handler");
    assert_eq!(checkpoint.handlers.handlers.len(), 1);
    let artifact = crate::api::codec::checkpoint_artifact_json(&[], "main", checkpoint).unwrap();
    let restored = crate::api::codec::checkpoint_from_json(&artifact, &checked)
        .expect("active handler must agree with the machine boundary");
    let resumed_host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
    ]));
    let resumed = Interpreter
        .resume_checkpoint(&checked, &restored, &resumed_host, model_options())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(
        resumed.value(),
        Some(&InterpValue::String("model-handled".into()))
    );
    assert_eq!(resumed_host.model_call_count(), 0);
}

use super::*;
use crate::{api::RunOptions, orchestration::WorkflowEvent, value::AgentRevisionValue};

#[tokio::test(flavor = "current_thread")]
async fn caller_retry_recovers_after_a_host_failure_in_another_checked_revision() {
    let body = r#"
        let Catch: ![Error<string> => [] for string] = handler {
            Error<string>.raise(error) => { finish error; }
        };
        return handle { perform infer(Prompt.new().user(Public(input))) } with Catch;
        "#;
    let base = frozen(
        &(r#"
module app;
import std.agent.prompt.Prompt;
import std.runtime.limits.Attempts;
agent Worker(input: string) -> string {
    BODY
}
flow main(worker: Worker) -> string {
    var output = "";
    retry { output = worker.run("draft"); }
    return output + ":caller";
}
flow inspect() -> unit { let definition = ^Worker; return; }
"#
        .replace("BODY", body)),
    );
    // Compile the original definition into a distinct checked revision. This
    // exercises execution ownership without changing the agent's contract.
    let checked = base.checked().clone();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.publication",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    let reflection = *checked.types.reflections.keys().next().unwrap();
    let catalog =
        Arc::new(DefinitionCatalog::build(checked, &origins, Default::default()).unwrap());
    let store = etas_frontend::code::definition::NativeDefinitionStore::new(
        catalog,
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    );
    let definition = store.reflect_agent(reflection).unwrap();
    let candidate = store
        .compile_definition(&definition, &mut FrontendSession::new(), Default::default())
        .unwrap();
    let prepared = InterpreterCandidatePublisher.publish(candidate).unwrap();
    let original = prepared.type_bridge().original_program().clone();
    let agent = AgentRevisionValue::new_in_program(
        prepared.clone(),
        None,
        &original,
        prepared.targets.agent_component().unwrap(),
    )
    .unwrap();
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
        model: true,
        ..Default::default()
    });
    host.seed_model_error(
        etas_host::HostErrorCode::ProviderUnavailable,
        "transient model outage",
    );
    host.seed_model_response_text("recovered");
    let mut options = RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(etas_host::HostActionGrant::allow("Agentic", "infer"));
    options.model_policy = crate::api::ModelExecutionPolicy {
        provider: Some(etas_host::ModelProviderId("fixture".into())),
        model: etas_host::ModelName("fixture-model".into()),
        provider_capabilities: Some(crate::testing::full_model_capabilities()),
        ..Default::default()
    };
    let result = Interpreter
        .run_checked(
            &original,
            EntryPoint {
                item: original.entry.unwrap(),
            },
            vec![crate::value::InterpValue::Callable(
                crate::control::CallTarget::AgentRevision(agent),
            )],
            &host,
            options,
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&crate::value::InterpValue::String(
            "recovered:caller".into()
        ))
    );
    assert_eq!(host.model_call_count(), 2);
    assert_eq!(
        result
            .events
            .iter()
            .filter(|event| matches!(event, WorkflowEvent::RetryAttemptFailed(_)))
            .count(),
        1
    );
    assert_eq!(
        result
            .events
            .iter()
            .filter(|event| matches!(event, WorkflowEvent::RetryAttemptSucceeded(_)))
            .count(),
        1
    );
}

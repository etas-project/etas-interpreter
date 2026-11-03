use super::*;
use etas_code::{PackageKey, syntax::CodeSyntaxOperations};
use etas_frontend::{
    EntryPolicy, Frontend, FrontendSession, ProjectInput, SourceInput,
    code::{
        definition::{DefinitionCatalog, FrozenDefinitionEnvironment},
        draft::DefaultBodyOverlay,
        identity::DefinitionOrigins,
        service::NativeCodeSyntaxOperations,
    },
};
use std::collections::BTreeMap;

mod bridge;
mod callables;
mod checkpoint;
mod checkpoint_nominal;
mod flows;
mod native_actions;
mod origins;
mod source_bindings;

fn frozen(source: &str) -> Arc<FrozenDefinitionEnvironment> {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
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
    DefinitionCatalog::build(checked, &origins, Default::default())
        .unwrap()
        .freeze_agent(reflection)
        .unwrap()
}

fn compile(frozen: Arc<FrozenDefinitionEnvironment>, body: &str) -> CheckedCandidate {
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay =
        DefaultBodyOverlay::prepare(frozen, syntax.parse_body(body).unwrap(), &syntax).unwrap();
    FrontendSession::new()
        .compile_body_overlay(overlay)
        .unwrap()
}

const SOURCE: &str = "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }";

#[test]
fn publisher_runs_the_real_plan_pipeline_for_the_candidate_program() {
    let candidate = compile(frozen(SOURCE), "return 2;");
    let revision = candidate.revision().clone();
    let identity = candidate.identity().clone();
    let original = candidate.frozen().checked().clone();
    let target = candidate.entry();
    let program = candidate.program().clone();
    let prepared = InterpreterCandidatePublisher.publish(candidate).unwrap();
    assert_eq!(prepared.identity(), &identity);
    assert_eq!(prepared.revision(), &revision);
    assert_eq!(prepared.entry().item, target);
    assert!(Arc::ptr_eq(prepared.program(), &program));
    assert!(Arc::ptr_eq(
        prepared.type_bridge().original_program(),
        &original
    ));
    assert!(Arc::ptr_eq(
        prepared.type_bridge().candidate_program(),
        &program
    ));
    assert!(prepared.plan().diagnostics.is_empty());
    assert!(prepared.plan().slots.slot_count() > 0);
}

#[test]
fn publication_does_not_execute_a_raising_candidate() {
    let source = "module app; agent Worker() -> i32 ![Error<string>] { return perform Error<string>.raise(\"original\"); } flow inspect() -> unit { let d = ^Worker; return; }";
    let candidate = compile(
        frozen(source),
        "return perform Error<string>.raise(\"do not execute while publishing\");",
    );
    let prepared = InterpreterCandidatePublisher.publish(candidate).unwrap();
    assert!(prepared.plan().diagnostics.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn prepared_nominal_results_keep_the_checked_revision_type_correspondence() {
    let frozen = frozen(
        "module app; type UserId = string; type Profile = { id: UserId }; agent Worker(value: UserId) -> Profile { return Profile { id = value }; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let candidate = compile(frozen.clone(), "return Profile { id = value };");
    let callable = frozen.contract().entry().unwrap();
    let scope = etas_code::descriptor::GenericOwner::Method(callable.as_method().unwrap().clone());
    let signature = |program: &CheckedProgram, item| match &program.types.item_signatures[&item] {
        etas_types::ItemSignature::Flow(signature) => signature.clone(),
        etas_types::ItemSignature::Agent(agent) => agent.callable().unwrap().clone(),
        _ => panic!("checked callable"),
    };
    let original = signature(frozen.checked(), frozen.default_body().unwrap());
    let prepared = InterpreterCandidatePublisher.publish(candidate).unwrap();
    let bridge = prepared.type_bridge();
    let parameter = bridge
        .to_candidate(frozen.checked(), &scope, original.params[0])
        .unwrap();
    let output = bridge
        .to_candidate(frozen.checked(), &scope, original.output)
        .unwrap();
    let argument = crate::api::InterpValue::Nominal {
        ty: original.params[0],
        value: crate::api::InterpValue::String("u1".into()).into(),
    };
    let argument = bridge
        .value_to_candidate(frozen.checked(), &scope, original.params[0], argument)
        .unwrap();
    let host = crate::testing::FakeHost::new(Default::default());
    let result = prepared
        .create_run(vec![argument], &host, crate::api::RunOptions::default())
        .execute()
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(crate::api::InterpValue::Nominal { ty, value }) = result.value() else {
        panic!("nominal result")
    };
    assert_eq!(*ty, output);
    assert_eq!(
        bridge.to_original(prepared.program(), &scope, *ty).unwrap(),
        original.output
    );
    let crate::api::InterpValue::Record(fields) = value.as_ref() else {
        panic!("record representation")
    };
    assert!(
        matches!(fields.get("id"), Some(crate::api::InterpValue::Nominal { ty, .. }) if ty == parameter)
    );
    let returned = bridge
        .value_to_original(
            prepared.program(),
            &scope,
            output,
            result.value().unwrap().clone(),
        )
        .unwrap();
    let crate::api::InterpValue::Nominal { ty, value } = returned else {
        panic!("original nominal result")
    };
    assert_eq!(ty, original.output);
    let crate::api::InterpValue::Record(fields) = value.as_ref() else {
        panic!("original record representation")
    };
    assert!(
        matches!(fields.get("id"), Some(crate::api::InterpValue::Nominal { ty, .. }) if ty == original.params[0])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn inline_revision_call_keeps_its_program_across_a_console_host_suspension() {
    let frozen = frozen(
        "module app; import std.io.println; agent Worker() -> i32 { println(\"original\")?; return 1; } flow main(value: Worker) -> i32 { return value(); } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let original = frozen.checked().clone();
    let prepared = InterpreterCandidatePublisher
        .publish(compile(frozen, "println(\"candidate\")?; return 2;"))
        .unwrap();
    let agent = crate::value::AgentRevisionValue::new_in_program(
        prepared.clone(),
        None,
        &original,
        prepared.targets.agent_component().unwrap(),
    )
    .unwrap();
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
        console: true,
        ..Default::default()
    });
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
            Default::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&crate::value::InterpValue::i32(2)));
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn inline_revision_rejects_equal_local_ids_from_an_unrelated_checked_program() {
    let source = "module app; agent Worker() -> i32 { return 1; } flow main(value: Worker) -> i32 { return value(); } flow inspect() -> unit { let d = ^Worker; return; }";
    let first = frozen(source);
    let original = first.checked().clone();
    let unrelated = frozen(source).checked().clone();
    assert!(!Arc::ptr_eq(&original, &unrelated));
    assert_eq!(original.entry, unrelated.entry);
    let prepared = InterpreterCandidatePublisher
        .publish(compile(first, "return 2;"))
        .unwrap();
    let agent = crate::value::AgentRevisionValue::new_in_program(
        prepared.clone(),
        None,
        &original,
        prepared.targets.agent_component().unwrap(),
    )
    .unwrap();
    let host = crate::testing::FakeHost::new(Default::default());
    let result = Interpreter
        .run_checked(
            &unrelated,
            EntryPoint {
                item: unrelated.entry.unwrap(),
            },
            vec![crate::value::InterpValue::Callable(
                crate::control::CallTarget::AgentRevision(agent),
            )],
            &host,
            Default::default(),
        )
        .await
        .unwrap();
    assert!(result.value().is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Analysis(
                etas_core::AnalysisDiagnosticCode::InvalidArguments
            )
            && diagnostic.message.contains("different checked program")),
        "{:?}",
        result.diagnostics
    );
    assert!(result.events.is_empty());
    assert_eq!(host.console_call_count(), 0);
}

#[test]
fn published_revisions_pin_separate_programs_without_changing_the_nominal_agent() {
    let frozen = frozen(SOURCE);
    let original = frozen.checked().clone();
    let first = InterpreterCandidatePublisher
        .publish(compile(frozen.clone(), "return 2;"))
        .unwrap();
    let second = InterpreterCandidatePublisher
        .publish(compile(frozen, "return 3;"))
        .unwrap();
    assert_eq!(first.identity(), second.identity());
    assert_ne!(first.revision().key(), second.revision().key());
    assert_ne!(first.snapshot_fingerprint(), second.snapshot_fingerprint());
    assert!(!Arc::ptr_eq(first.program(), second.program()));
    assert!(!Arc::ptr_eq(first.program(), &original));
    assert!(!Arc::ptr_eq(first.plan(), second.plan()));
    let alias = first.clone();
    let pinned_revision = first.revision().key();
    drop(first);
    assert_eq!(alias.revision().key(), pinned_revision);
    assert_ne!(alias.revision().key(), second.revision().key());
}

#[tokio::test(flavor = "current_thread")]
async fn explicitly_running_each_prepared_program_uses_its_own_revision() {
    let frozen = frozen(SOURCE);
    let first = InterpreterCandidatePublisher
        .publish(compile(frozen.clone(), "return 2;"))
        .unwrap();
    let second = InterpreterCandidatePublisher
        .publish(compile(frozen, "return 3;"))
        .unwrap();
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability::default());
    for (prepared, expected) in [(&first, 2), (&second, 3), (&first, 2)] {
        let result = prepared
            .create_run(Vec::new(), &host, crate::api::RunOptions::default())
            .execute()
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(
            result.value().cloned(),
            Some(crate::api::InterpValue::i32(expected))
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn preparation_and_execution_do_not_select_an_unrelated_main() {
    let source = "module app; agent Worker() -> i32 { return 1; } flow main() -> i32 { return abort(\"must not execute main\"); } flow inspect() -> unit { let d = ^Worker; return; }";
    let prepared = InterpreterCandidatePublisher
        .publish(compile(frozen(source), "return 2;"))
        .unwrap();
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability::default());
    let result = prepared
        .create_run(Vec::new(), &host, crate::api::RunOptions::default())
        .execute()
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(crate::api::InterpValue::i32(2))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_bare_agents_checked_default_method_can_be_prepared_and_run() {
    let source = "module app; agent Worker; impl agent Worker { flow run() -> i32 { return 1; } } flow inspect() -> unit { let d = ^Worker; return; }";
    let prepared = InterpreterCandidatePublisher
        .publish(compile(frozen(source), "return 2;"))
        .unwrap();
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability::default());
    let result = prepared
        .create_run(Vec::new(), &host, crate::api::RunOptions::default())
        .execute()
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value().cloned(),
        Some(crate::api::InterpValue::i32(2))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn running_a_prepared_revision_consumes_its_plan_and_rejects_an_entry_mismatch() {
    let mut prepared = InterpreterCandidatePublisher
        .publish(compile(frozen(SOURCE), "return 2;"))
        .unwrap();
    let revision = Arc::get_mut(&mut prepared).unwrap();
    Arc::make_mut(&mut revision.plan).entry.item = etas_hir::HirItemId(u32::MAX);
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability::default());
    let result = prepared
        .create_run(Vec::new(), &host, crate::api::RunOptions::default())
        .execute()
        .await
        .unwrap();
    assert!(result.value().is_none());
    assert!(result.diagnostics.iter().any(|diagnostic| diagnostic.code
        == etas_core::DiagnosticCode::Analysis(
            etas_core::AnalysisDiagnosticCode::InvalidArguments
        )));
}

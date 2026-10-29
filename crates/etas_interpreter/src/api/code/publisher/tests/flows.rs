use super::*;
mod checkpoint;

fn flow_candidate(body: &str) -> CheckedCandidate {
    let source = "module app; flow normalize(text: string) -> string ![] { return std.text.trim(text); } flow inspect() -> unit { let definition = ^normalize; return; }";
    flow_candidate_for(source, body)
}

fn flow_candidate_for(source: &str, body: &str) -> CheckedCandidate {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.flow-publication",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    let expr = *checked.types.reflections.keys().next().unwrap();
    let frozen = DefinitionCatalog::build(checked, &origins, Default::default())
        .unwrap()
        .freeze_flow(expr)
        .unwrap();
    compile(frozen, body)
}

#[tokio::test(flavor = "current_thread")]
async fn published_flow_keeps_nominal_arguments_and_results_across_the_type_bridge() {
    use crate::api::InterpValue;
    let candidate = flow_candidate_for(
        "module app; type UserId = string; flow identity(value: UserId) -> UserId ![] { return value; } flow inspect() -> unit { let definition = ^identity; return; }",
        "let copied = value; return copied;",
    );
    let frozen = candidate.frozen().clone();
    let scope = etas_code::descriptor::GenericOwner::Declaration(candidate.identity().clone());
    let etas_types::ItemSignature::Flow(signature) =
        &frozen.checked().types.item_signatures[&frozen.default_body().unwrap()]
    else {
        panic!("flow signature");
    };
    let source_type = signature.output;
    let prepared = InterpreterCandidatePublisher.publish(candidate).unwrap();
    let bridge = prepared.type_bridge();
    let argument = InterpValue::Nominal {
        ty: source_type,
        value: InterpValue::String("u1".into()).into(),
    };
    let argument = bridge
        .value_to_candidate(frozen.checked(), &scope, source_type, argument)
        .unwrap();
    let host = crate::testing::FakeHost::new(Default::default());
    let result = prepared
        .create_run(vec![argument], &host, Default::default())
        .execute()
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let value = result.value().cloned().unwrap();
    let candidate_type = bridge
        .to_candidate(frozen.checked(), &scope, source_type)
        .unwrap();
    let returned = bridge
        .value_to_original(prepared.program(), &scope, candidate_type, value)
        .unwrap();
    assert_eq!(
        returned,
        InterpValue::Nominal {
            ty: source_type,
            value: InterpValue::String("u1".into()).into()
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn ordinary_flow_candidate_is_published_and_runs_without_an_agent_component() {
    let candidate =
        flow_candidate("let cleaned = std.text.trim(text); return std.text.uppercase(cleaned);");
    assert_eq!(candidate.identity().kind(), etas_code::DefinitionKind::Flow);
    let prepared = InterpreterCandidatePublisher.publish(candidate).unwrap();
    assert!(prepared.targets.agent_component().is_none());
    assert!(prepared.targets.agent_item().is_none());
    let host = crate::testing::FakeHost::new(Default::default());
    let result = prepared
        .create_run(
            vec![crate::api::InterpValue::String("  updated  ".into())],
            &host,
            Default::default(),
        )
        .execute()
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&crate::api::InterpValue::String("UPDATED".into()))
    );
    assert_eq!(host.console_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn published_flow_value_uses_ordinary_higher_order_dispatch_with_its_pinned_program() {
    let candidate = flow_candidate_for(
        "module app; flow normalize(text: string) -> string ![] { return std.text.trim(text); } flow main(next: (string) -> string ![]) -> string ![] { return next(\"  updated  \"); } flow inspect() -> unit { let definition = ^normalize; return; }",
        "let cleaned = std.text.trim(text); return std.text.uppercase(cleaned);",
    );
    let original = candidate.frozen().checked().clone();
    let entry = original.entry.unwrap();
    let etas_types::ItemSignature::Flow(signature) = &original.types.item_signatures[&entry] else {
        panic!("source entry signature");
    };
    let expected = signature.params[0];
    let prepared = InterpreterCandidatePublisher.publish(candidate).unwrap();
    let value =
        crate::value::ScopedCallableValue::from_revision(prepared, None, &original, expected)
            .unwrap();
    let host = crate::testing::FakeHost::new(Default::default());
    let result = Interpreter
        .run_checked(
            &original,
            EntryPoint { item: entry },
            vec![crate::api::InterpValue::Callable(value)],
            &host,
            Default::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&crate::api::InterpValue::String("UPDATED".into()))
    );
    assert_eq!(host.console_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn published_flow_value_does_not_accept_an_unrelated_expected_interface() {
    let candidate = flow_candidate_for(
        "module app; flow normalize(text: string) -> string ![] { return text; } flow main(next: (i32) -> i32 ![]) -> i32 ![] { return next(7); } flow inspect() -> unit { let definition = ^normalize; return; }",
        "return text;",
    );
    let original = candidate.frozen().checked().clone();
    let etas_types::ItemSignature::Flow(signature) =
        &original.types.item_signatures[&original.entry.unwrap()]
    else {
        panic!("source entry signature");
    };
    let expected = signature.params[0];
    let prepared = InterpreterCandidatePublisher.publish(candidate).unwrap();
    assert!(
        crate::value::ScopedCallableValue::from_revision(prepared, None, &original, expected)
            .is_err()
    );
}

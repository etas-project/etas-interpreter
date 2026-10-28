use super::*;
use etas_frontend::code::{definition::NativeDefinitionStore, identity::SourceModuleOrigin};

#[tokio::test(flavor = "current_thread")]
async fn canonical_source_origins_survive_publication_reflection_and_frozen_calls() {
    for reflection_result in [true, false] {
        let declaration = if reflection_result {
            "agent Worker() -> string { return \"baseline\"; }"
        } else {
            "agent Worker() -> i32 { return 1; }"
        };
        let source = frozen(&format!(
            "module dependency_alias.workers; {declaration} flow inspect() -> unit {{ let definition = ^Worker; return; }}"
        ));
        let checked = source.checked().clone();
        let package = source.identity().package();
        let origins = DefinitionOrigins {
            source_modules: checked
                .hir
                .modules
                .iter()
                .map(|module| {
                    (
                        *module,
                        SourceModuleOrigin::new(package, vec!["library".into(), "workers".into()])
                            .unwrap(),
                    )
                })
                .collect(),
            standard_package: package,
            external_definitions: BTreeMap::new(),
        };
        let expr = *checked.types.reflections.keys().next().unwrap();
        let catalog =
            Arc::new(DefinitionCatalog::build(checked, &origins, Default::default()).unwrap());
        let definitions = NativeDefinitionStore::new(
            catalog.clone(),
            Arc::new(NativeCodeSyntaxOperations::default()),
            Default::default(),
        );
        let baseline = definitions
            .reflect_agent(expr)
            .unwrap()
            .revision()
            .key()
            .fingerprint()
            .to_string();
        let body = if reflection_result {
            "let original = ^Worker; return original.revision.key;"
        } else {
            "return Worker();"
        };
        let prepared = InterpreterCandidatePublisher
            .publish(compile(catalog.freeze_agent(expr).unwrap(), body))
            .unwrap();
        assert_eq!(prepared.identity().module(), &["library", "workers"]);
        let host = crate::testing::FakeHost::new(Default::default());
        let result = prepared
            .create_run(vec![], &host, Default::default())
            .execute()
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let expected = if reflection_result {
            crate::value::InterpValue::String(baseline.into())
        } else {
            crate::value::InterpValue::i32(1)
        };
        assert_eq!(result.value(), Some(&expected));
        assert!(
            result
                .events
                .iter()
                .all(|event| !matches!(event, crate::orchestration::WorkflowEvent::HostTrace(_)))
        );
        assert_eq!(host.console_call_count(), 0);
    }
}

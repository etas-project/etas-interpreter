use super::*;

async fn invoke_guarded_revision(reflected: &str, fail_guard: bool, edit_body: bool) {
    invoke_guarded_revision_with_handler_actions(reflected, fail_guard, edit_body, false).await;
}

async fn invoke_guarded_revision_with_handler_actions(
    reflected: &str,
    fail_guard: bool,
    edit_body: bool,
    nested_actions: bool,
) {
    use etas_code::{
        definition::CodeDefinitionOperations, report::CheckStatus, syntax::CodeSyntaxOperations,
    };
    use etas_frontend::{
        Frontend, FrontendSession,
        code::{
            definition::{DefinitionCatalog, NativeDefinitionStore, ReflectionOptions},
            identity::DefinitionOrigins,
            service::{CandidatePublisher, NativeCodeSyntaxOperations},
        },
    };
    use std::{collections::BTreeMap, sync::Arc};
    let source = r#"
module app.main;
effect Gate { action check() -> unit; action send() -> unit; }
spec Safe: trace = Gate.check >> Gate.send;
flow guarded() -> i32 ![] ~ Safe {
    handle { perform Gate.check(); perform Gate.send(); } with {
        Gate.check() => { resume (); }
        Gate.send() => { resume (); }
    };
    return 7;
}
flow inspect() -> unit { let definition = ^guarded; return; }
flow wrapper() -> i32 ![] { return guarded(); }
flow main(current: () -> i32 ![]) -> i32 { return current(); }
"#;
    let source = if fail_guard {
        FAILED_GUARD_THEN_SEND.replace("flow main()", "flow guarded()")
            + "\nflow inspect() -> unit { let definition = ^guarded; return; }\nflow main(current: () -> i32 ![]) -> i32 { return current(); }"
    } else {
        source.to_owned()
    };
    let source = if nested_actions {
        source
            .replace("effect Gate", "effect Audit { action note() -> unit; } effect Gate")
            .replace("spec Safe: trace = Gate.check >> Gate.send;", "spec Safe: trace = (Gate.check >> Gate.send) & +Audit.note;")
            .replace("Gate.check() => { resume (); }", "Gate.check() => { handle { perform Audit.note(); } with { Audit.note() => { resume (); } }; resume (); }")
    } else {
        source
    };
    let source = source.replace("^guarded", &format!("^{reflected}"));
    let mut input = project::residual_project_input(&source);
    input.options.reflection = ReflectionOptions {
        retain_bodies: vec![format!("app.main.{reflected}")],
        export_bodies: vec![],
    };
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let expression = *checked.types.reflections.keys().next().unwrap();
    let package = etas_code::PackageKey::from_name_and_version("app", "1.0.0").unwrap();
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    let catalog =
        Arc::new(DefinitionCatalog::build(checked.clone(), &origins, Default::default()).unwrap());
    let store = Arc::new(NativeDefinitionStore::new(
        catalog,
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    ));
    let original = store.reflect_flow(expression).unwrap();
    let body = store.resolve(&original).unwrap().body().unwrap().clone();
    let body = if edit_body {
        let text = store.syntax_operations().format(&body).unwrap();
        let text = text.replace("return 7;", "return 9;");
        store
            .syntax_operations()
            .parse_body(&format!("let edited = true; {text}"))
            .unwrap()
    } else {
        body
    };
    let draft = store.with_body(&original, body).unwrap();
    let analysis = FrontendSession::new()
        .analyze_body_overlay(store.body_overlay(&draft).unwrap())
        .unwrap();
    assert_eq!(
        analysis.check_report(Default::default()).unwrap().status,
        CheckStatus::Guarded
    );
    let candidate = analysis.into_checked_candidate(Default::default()).unwrap();
    let revision = crate::api::code::InterpreterCandidatePublisher
        .publish(candidate)
        .unwrap();
    let entry = checked.entry.unwrap();
    let etas_types::ItemSignature::Flow(signature) = &checked.types.item_signatures[&entry] else {
        panic!("source entry")
    };
    let current = crate::value::ScopedCallableValue::from_revision(
        revision,
        None,
        &checked,
        signature.params[0],
    )
    .unwrap();
    let host = FakeHost::new(availability(&[]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint { item: entry },
            vec![value::InterpValue::Callable(current)],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    if fail_guard {
        assert!(result.value().is_none());
        assert!(
            residual_violation(&result)
                .is_some_and(|diagnostic| diagnostic.message.contains("Gate.send")),
            "{:#?}",
            result.diagnostics
        );
    } else {
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
        let expected = if edit_body && reflected == "guarded" {
            9
        } else {
            7
        };
        assert_eq!(result.value(), Some(&value::InterpValue::i32(expected)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn guarded_reflected_flow_is_installed_and_invoked_through_its_native_revision() {
    invoke_guarded_revision("guarded", false, false).await;
}

#[tokio::test(flavor = "current_thread")]
async fn reflected_wrapper_installs_its_callee_guard_with_the_callee_revision() {
    invoke_guarded_revision("wrapper", false, false).await;
}

#[tokio::test(flavor = "current_thread")]
async fn guarded_reflected_flow_rejects_failed_evidence_before_the_target_dispatch() {
    invoke_guarded_revision("guarded", true, false).await;
}

#[tokio::test(flavor = "current_thread")]
async fn edited_guarded_flow_installs_fresh_analysis_and_executes_its_revision() {
    invoke_guarded_revision("guarded", false, true).await;
}

#[tokio::test(flavor = "current_thread")]
async fn edited_wrapper_preserves_its_callees_independent_residual_revision() {
    invoke_guarded_revision("wrapper", false, true).await;
}

#[tokio::test(flavor = "current_thread")]
async fn edited_guarded_flow_executes_nested_handler_actions_with_shared_trace_facts() {
    invoke_guarded_revision_with_handler_actions("guarded", false, true, true).await;
}

#[tokio::test(flavor = "current_thread")]
async fn wrapper_without_a_lexical_spec_executes_its_guarded_callee_scope() {
    let source = ORDERED_HANDLED.replace("flow main()", "flow guarded()")
        + "\nflow main() -> i32 ![] { return guarded(); }";
    let checked = residual_checked(&source);
    let result = run(&checked, &FakeHost::new(availability(&[]))).await;
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(1)));
}

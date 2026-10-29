use super::*;

#[tokio::test(flavor = "current_thread")]
async fn compiled_flow_preserves_boolean_resume_value() {
    use crate::api::code::InterpreterCandidatePublisher;
    use etas_code::{definition::CodeDefinitionOperations, syntax::CodeSyntaxOperations};
    use etas_frontend::{FrontendSession, code::service::CandidatePublisher};

    let (checked, definitions) = prepared(
        r#"
module app.main;
effect Choice { action decide() -> bool; }
flow normalize(f: () -> bool ![Choice.decide]) -> bool ![] {
    let result = handle { f() } with { Choice.decide() => { resume true; } };
    return result;
}
flow callback() -> bool ![Choice.decide] { return perform Choice.decide(); }
flow inspect() -> unit { let definition = ^normalize; return; }
flow main(current: ((() -> bool ![Choice.decide]) -> bool ![])) -> bool {
    return current(callback);
}
"#,
        &["app.main.normalize"],
    );
    let reflection = *checked.types.reflections.keys().next().unwrap();
    let backend = definitions.definitions().clone();
    let original = backend.reflect_flow(reflection).unwrap();
    let body = backend.syntax_operations().parse_body(
        "let result = handle { f() } with { Choice.decide() => { resume true; } }; return result;",
    ).unwrap();
    let draft = backend.with_body(&original, body).unwrap();
    let candidate = FrontendSession::new()
        .compile_body_overlay(backend.body_overlay(&draft).unwrap())
        .unwrap();
    let mut revision = InterpreterCandidatePublisher.publish(candidate).unwrap();
    Arc::get_mut(&mut revision)
        .unwrap()
        .retain_reflected_definition(backend.clone(), &draft)
        .unwrap();
    let entry = checked.entry.unwrap();
    let etas_types::ItemSignature::Flow(signature) = &checked.types.item_signatures[&entry] else {
        panic!("source entry");
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
            vec![InterpValue::Callable(current)],
            &host,
            RunOptions {
                code_definitions: Some(definitions),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiled_flow_preserves_a_resuming_callback_handler() {
    use crate::api::code::InterpreterCandidatePublisher;
    use etas_code::{definition::CodeDefinitionOperations, syntax::CodeSyntaxOperations};
    use etas_frontend::{FrontendSession, code::service::CandidatePublisher};

    let (checked, definitions) = prepared(
        r#"
module app.main;
flow normalize(f: () -> unit ![Console.stdout_write]) -> unit ![] {
    handle { f(); } with { Console.stdout_write(_) => { resume; } };
    return;
}
flow callback() -> unit ![Console.stdout_write] {
    perform Console.stdout_write("must be intercepted");
    return;
}
flow inspect() -> unit { let definition = ^normalize; return; }
flow main(current: ((() -> unit ![Console.stdout_write]) -> unit ![])) -> bool {
    current(callback);
    return true;
}
"#,
        &["app.main.normalize"],
    );
    let reflection = *checked.types.reflections.keys().next().unwrap();
    let backend = definitions.definitions().clone();
    let original = backend.reflect_flow(reflection).unwrap();
    let body = backend
        .syntax_operations()
        .parse_body("handle { f(); } with { Console.stdout_write(_) => { resume; } }; return;")
        .unwrap();
    let draft = backend.with_body(&original, body).unwrap();
    let candidate = FrontendSession::new()
        .compile_body_overlay(backend.body_overlay(&draft).unwrap())
        .unwrap();
    let mut revision = InterpreterCandidatePublisher.publish(candidate).unwrap();
    Arc::get_mut(&mut revision)
        .unwrap()
        .retain_reflected_definition(backend.clone(), &draft)
        .unwrap();
    let entry = checked.entry.unwrap();
    let etas_types::ItemSignature::Flow(signature) = &checked.types.item_signatures[&entry] else {
        panic!("source entry");
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
            vec![InterpValue::Callable(current)],
            &host,
            RunOptions {
                code_definitions: Some(definitions),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(host.console_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn ordinary_flow_diff_compares_snapshots_without_compiling_or_inventing_analysis() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.{parse_body, diff};
flow target(text: string) -> string ![] { return abort("diff must not invoke this flow"); }
flow main() -> bool {
    let original = ^target;
    let syntax = match parse_body("return 42;") { Ok(body) => body, Err(_) => { return false; }, };
    let draft = original.with_body(syntax);
    let changed = diff(original, draft);
    let unchanged = diff(original, original);
    return changed.schema == "etas.code.change-report.v2"
        && changed.identity == original.dependencies.lexical_owner
        && changed.body_changed == Some(true)
        && changed.old_body_available && changed.new_body_available
        && !changed.signature_changed && !changed.dependencies_changed
        && changed.escaping_changed == None && changed.requested_changed == None
        && changed.default_handled_changed == None && changed.trace_changed == None
        && changed.determinism_changed == None
        && unchanged.body_changed == Some(false)
        && unchanged.escaping_changed == Some(false)
        && unchanged.requested_changed == Some(false);
}
"#,
        &["app.main.target"],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn flow_diff_without_retained_bodies_compares_available_checked_analysis() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.diff;
flow target() -> i32 ![] { return abort("diff must not execute its target"); }
flow main() -> bool {
    let definition = ^target;
    let report = diff(definition, definition);
    return definition.body == None && !report.old_body_available && !report.new_body_available
        && report.body_changed == None && definition.effects.requested != None
        && report.requested_changed == Some(false) && report.escaping_changed == Some(false);
}
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn flow_diff_does_not_confuse_equal_signatures_with_equal_definition_identities() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.diff;
flow first() -> i32 ![] { return abort("diff must not invoke first"); }
flow second() -> i32 ![] { return abort("diff must not invoke second"); }
flow main() -> unit { let changes = diff(^first, ^second); return; }
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Analysis(
                etas_core::AnalysisDiagnosticCode::InvalidArguments
            )),
        "{:?}",
        result.diagnostics
    );
    assert!(result.value().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn compiled_flow_reflection_reports_new_analysis_with_the_original_contract_and_body_access()
{
    use crate::api::code::InterpreterCandidatePublisher;
    use etas_code::{definition::CodeDefinitionOperations, syntax::CodeSyntaxOperations};
    use etas_frontend::{FrontendSession, code::service::CandidatePublisher};

    for retain_body in [false, true] {
        let retained: &[&str] = if retain_body {
            &["app.main.target"]
        } else {
            &[]
        };
        let (checked, definitions) = prepared(
            r#"
module app.main;
import std.compiler.{CallableDef, ChangeReport, diff};
import std.io.println;
flow target(flag: bool) -> bool ![] {
    if flag { println("base request")?; }
    return flag;
}
flow inspect() -> unit { let definition = ^target; return; }
flow main(current: (bool) -> bool ![]) -> (CallableDef<(bool) -> bool ![]>, ChangeReport) {
    let current_definition = ^current;
    return (current_definition, diff(^target, current_definition));
}
"#,
            retained,
        );
        let reflection = checked
            .types
            .reflections
            .iter()
            .find_map(|(expr, fact)| {
                matches!(
                    fact.target,
                    etas_types::CheckedReflectionTarget::Callable { .. }
                )
                .then_some(*expr)
            })
            .unwrap();
        let backend = definitions.definitions().clone();
        let original = backend.reflect_flow(reflection).unwrap();
        let view = backend.resolve(&original).unwrap();
        let base_analysis = view.current_analysis().unwrap();
        assert!(
            !view
                .current_analysis_descriptors()
                .unwrap()
                .effect_row(base_analysis.effects.requested.unwrap())
                .unwrap()
                .effects
                .is_empty()
        );
        let frozen_contract = view.environment().unwrap().contract().fingerprint();
        let body = backend
            .syntax_operations()
            .parse_body("return flag;")
            .unwrap();
        let draft = backend.with_body(&original, body).unwrap();
        let candidate = FrontendSession::new()
            .compile_body_overlay(backend.body_overlay(&draft).unwrap())
            .unwrap();
        let mut revision = InterpreterCandidatePublisher.publish(candidate).unwrap();
        Arc::get_mut(&mut revision)
            .unwrap()
            .retain_reflected_definition(backend.clone(), &draft)
            .unwrap();
        let entry = checked.entry.unwrap();
        let etas_types::ItemSignature::Flow(signature) = &checked.types.item_signatures[&entry]
        else {
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
                vec![InterpValue::Callable(current)],
                &host,
                RunOptions {
                    code_definitions: Some(definitions),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let Some(InterpValue::Tuple(values)) = result.value() else {
            panic!("compiled flow reflection and diff")
        };
        let InterpValue::CallableDefinition(reflected) = &values[0] else {
            panic!("compiled flow reflection")
        };
        let InterpValue::Nominal { ty, value } = &values[1] else {
            panic!("nominal diff report");
        };
        let Some(etas_types::Type::Tuple(result_types)) = checked.type_store.get(signature.output)
        else {
            panic!("checked tuple result");
        };
        assert_eq!(*ty, result_types[1]);
        let InterpValue::Record(report) = &**value else {
            panic!("diff report representation");
        };
        assert_eq!(
            report.get("requested_changed"),
            Some(InterpValue::OptionSome(InterpValue::Bool(true).into()))
        );
        assert_eq!(
            report.get("body_changed"),
            Some(if retain_body {
                InterpValue::OptionSome(InterpValue::Bool(true).into())
            } else {
                InterpValue::OptionNone
            })
        );
        let Some(InterpValue::CodeMetadata(identity)) = report.get("identity") else {
            panic!("neutral identity");
        };
        assert!(
            matches!(identity.payload(), crate::value::CodeMetadataPayload::Definition(value) if value.kind() == etas_code::DefinitionKind::Flow)
        );
        let (graph, analysis) = reflected.analysis().unwrap();
        assert!(
            graph
                .effect_row(analysis.effects.requested.unwrap())
                .unwrap()
                .effects
                .is_empty()
        );
        let current = reflected
            .backend()
            .resolve(reflected.definition().unwrap())
            .unwrap();
        assert_eq!(
            current.environment().unwrap().contract().fingerprint(),
            frozen_contract
        );
        assert_ne!(current.revision(), view.revision());
        assert_eq!(reflected.body().is_some(), retain_body);
        assert_eq!(host.console_call_count(), 0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ordinary_flow_draft_is_immutable_and_does_not_reuse_baseline_analysis() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.parse_body;
flow normalize(text: string) -> string ![] { return text; }
flow main() -> bool {
    let original = ^normalize;
    let body = match parse_body("return \"changed\";") { Ok(body) => body, Err(_) => { return false; }, };
    let draft = original.with_body(body);
    return original.id == draft.id && original.signature == draft.signature
        && original.dependencies == draft.dependencies
        && original.revision.key != draft.revision.key
        && match original.effects.escaping { Some(_) => true, None => false, }
        && match draft.effects.declared_bound { Some(_) => true, None => false, }
        && match draft.effects.escaping { None => true, Some(_) => false, }
        && match draft.determinism { None => true, Some(_) => false, }
        && match draft.trace { None => true, Some(_) => false, }
        && match original.body { Some(body) => body.text != "", None => false, }
        && match draft.body { Some(body) => body.text != "", None => false, }
        && normalize("original") == "original";
}
"#,
        &["app.main.normalize"],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn current_flow_binding_can_be_edited_without_changing_its_old_alias() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.parse_body;
flow first(text: string) -> string ![] { return "first"; }
flow second(text: string) -> string ![] { return "second"; }
flow main() -> bool {
    var current = first;
    let old = current;
    let original = ^current;
    current = second;
    let selected = ^current;
    let body = match parse_body("return text;") { Ok(body) => body, Err(_) => { return false; }, };
    let draft = selected.with_body(body);
    return original.id != selected.id && selected.id == draft.id
        && selected.revision.key != draft.revision.key
        && old("x") == "first" && current("x") == "second";
}
"#,
        &["app.main.first", "app.main.second"],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn ordinary_flow_binding_reflection_follows_the_current_implementation() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
flow first(text: string) -> string ![] { return text; }
flow second(text: string) -> string ![] { return "second"; }
flow main() -> bool {
    var current = first;
    let original = (^current).id;
    current = second;
    return original != (^current).id;
}
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn erased_callable_array_preserves_heterogeneous_flow_metadata() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.CallableDef;
flow first(text: string) -> string ![] { return text; }
flow second(left: i32, right: i32) -> i32 ![] { return left + right; }
flow inspect(definitions: Array<CallableDef>) -> usize {
    let first = match definitions.get(0) {
        Some(definition) => definition.signature.parameters.len(),
        None => 0,
    };
    let second = match definitions.get(1) {
        Some(definition) => definition.signature.parameters.len(),
        None => 0,
    };
    return first + second;
}
flow main() -> usize { return inspect([^first, ^second]); }
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::usize(3)));
}

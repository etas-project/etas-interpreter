use super::super::*;
use etas_code::{CodeFingerprint, PackageKey};
use etas_frontend::{
    EntryPolicy, Frontend, ProjectInput, SourceInput,
    code::{
        definition::{DefinitionCatalog, NativeDefinitionStore, ReflectionOptions},
        identity::DefinitionOrigins,
        service::NativeCodeSyntaxOperations,
    },
};
use std::{collections::BTreeMap, sync::Arc};

mod applications;
mod cancel_edit;
mod checkpoints;
mod compile_handlers;
mod descriptors;
mod flows;
mod method_snapshots;

fn prepared(
    source: &str,
    retained: &[&str],
) -> (
    etas_frontend::CheckedProgram,
    Arc<crate::api::code::PreparedCodeDefinitions>,
) {
    prepared_project(
        ProjectInput::single_source(SourceInput::anonymous(source)),
        retained,
    )
}

fn prepared_project(
    mut input: ProjectInput,
    retained: &[&str],
) -> (
    etas_frontend::CheckedProgram,
    Arc<crate::api::code::PreparedCodeDefinitions>,
) {
    input.options.entry_policy = EntryPolicy::Optional;
    input.options.reflection = ReflectionOptions {
        retain_bodies: retained.iter().map(|name| (*name).into()).collect(),
        export_bodies: vec![],
    };
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let key = |name: &[u8]| {
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "etas.test.compiler-definitions",
            name,
        ))
    };
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, key(b"app")).unwrap(),
        standard_package: key(b"std"),
        external_definitions: BTreeMap::new(),
    };
    let catalog =
        Arc::new(DefinitionCatalog::build(checked.clone(), &origins, Default::default()).unwrap());
    let store = Arc::new(NativeDefinitionStore::new(
        catalog,
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    ));
    (
        checked,
        Arc::new(crate::api::code::PreparedCodeDefinitions::new(store)),
    )
}

async fn execute(checked: &etas_frontend::CheckedProgram, options: RunOptions) -> RunResult {
    let host = FakeHost::new(availability(&[]));
    let result = Interpreter
        .run_checked(
            checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options,
        )
        .await
        .unwrap();
    assert_eq!(host.console_call_count(), 0);
    assert!(host.model_requests().is_empty());
    assert!(host.tool_requests().is_empty());
    result
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_action_wrappers_visit_source_handlers_before_default_host_dispatch() {
    for (import, expression, action) in [
        ("std.compiler.check", "check(^Worker)", "check"),
        ("std.compiler.compile", "compile(^Worker)", "compile"),
    ] {
        let (checked, definitions) = prepared(
            &format!(
                r#"
module app.main;
import {import};
agent Worker() -> i32 {{ return abort("candidate must not execute"); }}
flow main() -> i32 {{
    return handle {{
        let result = {expression};
        return 99;
    }} with {{ Code.{action}(_) => {{ finish 7; }} }};
}}
"#
            ),
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
        assert_eq!(result.value(), Some(&InterpValue::i32(7)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_action_wrapper_finish_preserves_the_caller_continuation() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.compile;
agent Worker() -> i32 { return 7; }
flow intercepted() -> i32 {
    return handle {
        let implementation = compile(^Worker);
        return 99;
    } with { Code.compile(_) => { finish 7; } };
}
flow main() -> i32 { return intercepted() + 1; }
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
    assert_eq!(result.value(), Some(&InterpValue::i32(8)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_action_handlers_use_checked_identity_across_source_modules() {
    for expression in ["check(^Worker)", "perform Code.check(^Worker)"] {
        let mut input = ProjectInput::single_source(SourceInput::anonymous(
            r#"
module app.main;
import app.library.invoke;
flow main() -> i32 {
    return handle { invoke(); return 99; }
        with { Code.check(_) => { finish 7; } };
}
"#,
        ));
        let mut library = SourceInput::anonymous(format!(
            r#"
module app.library;
import std.compiler.check;
import std.compiler.CompileError;
agent Worker() -> i32 {{ return abort("must not execute"); }}
public flow invoke() -> unit ![Error<CompileError>] {{ let report = {expression}; return; }}
"#
        ));
        library.id = etas_core::SourceId(1);
        input.sources.push(library);
        let (checked, definitions) = prepared_project(input, &[]);
        let result = execute(
            &checked,
            RunOptions {
                code_definitions: Some(definitions),
                ..Default::default()
            },
        )
        .await;
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::i32(7)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_definitions_reflect_and_edit_real_syntax_without_executing_or_checking_it() {
    let (checked, prepared) = prepared(
        r#"
module app.main;
import std.compiler.parse_body;
agent Worker() -> i32 { return abort("must not execute"); }
flow main() -> bool {
    let original = ^Worker;
    match parse_body("return unresolved_name();") {
        Ok(syntax) => {
            let edited = original.with_body(syntax);
            match (original.body, edited.body) {
                (Some(before), Some(after)) => {
                    return before.text != after.text && after.text == "return unresolved_name();";
                },
                _ => { return false; },
            }
        },
        Err(_) => { return false; },
    }
}
"#,
        &["app.main.Worker"],
    );
    assert_eq!(checked.types.std_intrinsic_calls.len(), 1);
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(prepared),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_definitions_hidden_and_bare_agents_are_not_empty_source_fallbacks() {
    for declaration in ["agent Worker() -> i32 { return 1; }", "agent Worker;"] {
        let source = format!(
            r#"
module app.main;
import std.compiler.parse_body;
{declaration}
flow main() -> bool {{
    let original = ^Worker;
    match original.body {{ Some(_) => {{ return false; }}, None => {{}}, }}
    match parse_body("return 42;") {{
        Ok(syntax) => {{
            let draft = original.with_body(syntax);
            match draft.body {{ Some(body) => {{ return body.text == "return 42;"; }}, None => {{ return false; }}, }}
        }},
        Err(_) => {{ return false; }},
    }}
}}
"#
        );
        let (checked, prepared) = prepared(&source, &[]);
        let result = execute(
            &checked,
            RunOptions {
                code_definitions: Some(prepared),
                ..Default::default()
            },
        )
        .await;
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_definitions_callable_and_primary_body_projections_share_native_syntax() {
    let (checked, prepared) = prepared(
        r#"
module app.main;
agent Worker() -> i32 { return 1; }
flow helper() -> i32 { return 7; }
flow main() -> bool {
    let agent = ^Worker;
    let method = ^Worker.run;
    let flow_definition = ^helper;
    match (agent.body, method.body, flow_definition.body) {
        (Some(first), Some(second), Some(flow_body)) => {
            return first == second && flow_body.text == "return 7;";
        },
        _ => { return false; },
    }
}
"#,
        &["app.main.Worker", "app.main.helper"],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(prepared),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_definitions_missing_or_foreign_catalog_does_not_guess_package_identity() {
    let source =
        "module app.main; agent Worker; flow main() -> unit { let definition = ^Worker; return; }";
    let (checked, _) = prepared(source, &[]);
    let (_, foreign) = prepared(source, &[]);
    for provider in [None, Some(foreign)] {
        let result = execute(
            &checked,
            RunOptions {
                code_definitions: provider,
                ..Default::default()
            },
        )
        .await;
        assert!(result.value().is_none());
        assert!(
            result.diagnostics.iter().any(|diagnostic| matches!(
                diagnostic.code,
                DiagnosticCode::Analysis(
                    AnalysisDiagnosticCode::MissingCheckedFact
                        | AnalysisDiagnosticCode::InvalidArguments
                )
            )),
            "{:?}",
            result.diagnostics
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_definitions_generic_editing_uses_checked_abi_substitution() {
    let (checked, prepared) = prepared(
        r#"
module app.main;
import std.compiler.AgentDef;
import std.compiler.SyntaxBlock;
import std.compiler as code;
agent Worker() -> i32 { return 1; }
flow edit<A>(definition: AgentDef<A>, body: SyntaxBlock) -> AgentDef<A> {
    return definition.with_body(body);
}
flow main() -> bool {
    match code.parse_body("return 2;") {
        Ok(syntax) => {
            let edited = edit(^Worker, syntax);
            match edited.body { Some(body) => { return code.format(body) == "return 2;"; }, None => { return false; }, }
        },
        Err(_) => { return false; },
    }
}
"#,
        &["app.main.Worker"],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(prepared),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_definitions_cannot_leak_live_objects_into_host_or_checkpoint_codecs() {
    for (declaration, reflected, output) in [
        ("agent Worker;", "Worker", "AgentDef<Worker>"),
        ("flow helper() -> unit { return; }", "helper", "CallableDef"),
    ] {
        let source = format!(
            "module app.main; import std.compiler.AgentDef; import std.compiler.CallableDef; {declaration} flow main() -> {output} {{ return ^{reflected}; }}"
        );
        let (checked, prepared) = prepared(&source, &[]);
        let result = execute(
            &checked,
            RunOptions {
                code_definitions: Some(prepared),
                ..Default::default()
            },
        )
        .await;
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let value = result.value().unwrap();
        assert!(
            crate::eval::interp_to_host_value(value)
                .unwrap_err()
                .contains("not host-encodable")
        );
        assert!(
            crate::orchestration::ValueSnapshot::capture(value)
                .unwrap_err()
                .contains("locked definition environment")
        );
        assert!(
            crate::api::codec::value_from_json(&crate::api::codec::value_json(value).unwrap())
                .is_err()
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_definitions_injected_store_does_not_override_current_parse_limits() {
    use etas_code::syntax::CodeSyntaxOperations;
    let (checked, prepared) = prepared(
        r#"
module app.main;
import std.compiler.parse_body;
flow main() -> unit { let body = parse_body("return 42;"); return; }
"#,
        &[],
    );
    let cached = prepared
        .definitions()
        .syntax_operations()
        .parse_body("return 42;")
        .unwrap();
    for (source_bytes, tokens) in [(4, 100), (100, 2)] {
        let mut options = RunOptions {
            code_definitions: Some(prepared.clone()),
            ..Default::default()
        };
        options.syntax_limits.parse.max_source_bytes = source_bytes;
        options.syntax_limits.parse.max_tokens = tokens;
        let result = execute(&checked, options).await;
        assert!(result.value().is_none());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("limit")),
            "{:?}",
            result.diagnostics
        );
    }
    assert_eq!(
        prepared
            .definitions()
            .syntax_operations()
            .format(&cached)
            .unwrap(),
        "return 42;"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_definition_edit_arguments_suspend_and_resume_without_executing_twice() {
    let (checked, prepared) = prepared(
        r#"
module app.main;
import std.compiler.SyntaxBlock;
import std.compiler.parse_body;
import std.io.println;
agent Worker() -> i32 { return 1; }
flow replacement_body() -> SyntaxBlock ![Console, Error<IOError>] {
    println("edit");
    return match parse_body("return 2;") { Ok(body) => body, Err(_) => abort("bad test body"), };
}
flow main() -> bool ![Console, Error<IOError>] {
    let definition = ^Worker;
    let edited = definition.with_body(replacement_body());
    match edited.body { Some(body) => { return body.text == "return 2;"; }, None => { return false; }, }
}
"#,
        &["app.main.Worker"],
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let mut options = RunOptions {
        code_definitions: Some(prepared),
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
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(host.stdout_text(), "edit\n");
    assert_eq!(host.console_call_count(), 1);
    assert!(host.model_requests().is_empty());
}

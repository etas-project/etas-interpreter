use super::*;

#[tokio::test(flavor = "current_thread")]
async fn indirect_flow_compile_handler_uses_the_reachable_request_abi_without_a_service() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.compile;
import std.compiler.CallableDef;
import std.compiler.CompileError;
alias Task = () -> i32 ![];
flow target() -> i32 ![] { return 9; }
flow request(draft: CallableDef<Task>) -> Task ![Error<CompileError>] { return compile(draft); }
flow main() -> i32 {
    let selected = handle { request(^target) } with { Code.compile(_) => { resume target; } };
    return selected();
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
    assert_eq!(result.value(), Some(&InterpValue::i32(9)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_handler_finish_returns_its_own_captured_flow_without_claiming_compile_provenance()
{
    for completion in [
        "finish replacement;",
        "if true { finish replacement; } resume target;",
    ] {
        let (checked, definitions) = prepared(
            &format!(
                r#"
module app.main;
import std.compiler.compile;
flow target(value: i32) -> i32 ![] {{ return value; }}
flow alternative(value: i32) -> i32 ![] {{ return 9; }}
flow main() -> i32 {{
    let replacement = alternative;
    let selected = handle {{ compile(^target) }} with {{ Code.compile(_) => {{ {completion} }} }};
    return selected(7);
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
        assert!(
            result.diagnostics.is_empty(),
            "{completion}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.value(), Some(&InterpValue::i32(9)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_handler_resumes_the_original_checked_flow_without_a_compiler_service() {
    for expression in ["compile(^target)", "perform Code.compile(^target)"] {
        let (checked, definitions) = prepared(
            &format!(
                r#"
module app.main;
import std.compiler.compile;
flow target(value: i32) -> i32 ![] {{ return value; }}
flow main() -> i32 {{
    let candidate = handle {{ {expression} }} with {{ Code.compile(_) => {{ resume target; }} }};
    return candidate(7);
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
        assert!(
            result.diagnostics.is_empty(),
            "{expression}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.value(), Some(&InterpValue::i32(7)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_handler_rejects_an_unrelated_flow_even_with_the_same_callable_type() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.compile;
flow target(value: i32) -> i32 ![] { return value; }
flow unrelated(value: i32) -> i32 ![] { return value; }
flow main() -> i32 {
    let candidate = handle { compile(^target) } with {
        Code.compile(_) => { resume unrelated; }
    };
    return candidate(7);
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
    assert!(result.value().is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Analysis(
                etas_core::AnalysisDiagnosticCode::MissingCheckedFact
            )
            && diagnostic.message.contains("frozen-contract provenance")),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_handler_resumes_the_original_checked_agent_without_a_compiler_service() {
    for expression in ["compile(^Worker)", "perform Code.compile(^Worker)"] {
        let (checked, definitions) = prepared(
            &format!(
                r#"
module app.main;
import std.compiler.compile;
agent Worker() -> i32 {{ return 7; }}
flow main() -> i32 {{
    let candidate = handle {{ {expression} }} with {{ Code.compile(_) => {{ resume Worker; }} }};
    return candidate();
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
        assert!(
            result.diagnostics.is_empty(),
            "{expression}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.value(), Some(&InterpValue::i32(7)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_handler_result_guard_survives_host_suspension_before_resume() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.compile;
import std.io.println;
agent Worker() -> i32 { return 7; }
flow main() -> i32 {
    let candidate = handle { compile(^Worker) } with {
        Code.compile(_) => { println("handler")?; resume Worker; }
    };
    return candidate();
}
"#,
        &[],
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
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
    assert_eq!(result.value(), Some(&InterpValue::i32(7)));
    assert_eq!(host.stdout_text(), "handler\n");
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn discarded_compile_payload_does_not_drop_pending_contract_evidence_at_checkpoint() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.compile;
import std.runtime.checkpoint;
agent Worker() -> i32 { return 7; }
flow main() -> i32 {
    let candidate = handle { compile(^Worker) } with {
        Code.compile(_) => { checkpoint("pending guard"); resume Worker; }
    };
    return candidate();
}
"#,
        &[],
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
            RunOptions {
                code_definitions: Some(definitions),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.value().is_none());
    assert!(result.checkpoints.is_empty());
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("pending compiler contract evidence")),
        "{:?}",
        result.diagnostics
    );
}

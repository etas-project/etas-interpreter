use super::super::*;

#[test]
fn compiler_syntax_host_decode_cannot_invent_an_ast_from_projected_fields() {
    let checked = checked_project(
        r#"
module app.main;
import std.compiler.parse_body;
flow main() -> unit { let syntax = parse_body("return;"); return; }
"#,
    );
    let syntax = checked.types.known_std_types.code.syntax_block.unwrap();
    let error = crate::eval::host_to_typed_interp_value(
        HostValue::Record(vec![("text".into(), HostValue::String("return;".into()))]),
        syntax,
        &checked.type_store,
    )
    .unwrap_err();
    assert!(error.contains("no checked representation"), "{error}");
}

async fn run(source: &str, options: RunOptions) -> RunResult {
    let checked = checked_project(source);
    let host = FakeHost::new(availability(&[]));
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
    assert!(host.stdout_text().is_empty());
    result
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_syntax_parse_format_and_text_use_real_immutable_syntax() {
    let result = run(
        r#"
module app.main;
import std.compiler.parse_body;
import std.compiler.format;
flow main() -> bool {
    match parse_body("let answer=unresolved(42);return answer;") {
        Ok(syntax) => {
            let copy = syntax;
            let formatted = format(syntax);
            match parse_body(formatted) {
                Ok(again) => { return copy.text == formatted && format(again) == formatted; },
                Err(_) => { return false; },
            }
        },
        Err(_) => { return false; },
    }
}
"#,
        RunOptions::default(),
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_syntax_module_and_function_aliases_use_the_same_checked_intrinsics() {
    for (imports, parse, format_call) in [
        (
            "import std.compiler as code;",
            "code.parse_body",
            "code.format",
        ),
        (
            "import std.compiler.parse_body as parse; import std.compiler.format as render;",
            "parse",
            "render",
        ),
    ] {
        let source = format!(
            r#"
module app.main;
{imports}
flow main() -> bool {{
    match {parse}("return not_executed();") {{
        Ok(syntax) => {{ return {format_call}(syntax) == syntax.text; }},
        Err(_) => {{ return false; }},
    }}
}}
"#
        );
        let result = run(&source, RunOptions::default()).await;
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_syntax_invalid_source_returns_structured_diagnostics() {
    let result = run(
        r#"
module app.main;
import std.compiler.parse_body;
flow main() -> bool {
    match parse_body("let = ;") {
        Ok(_) => { return false; },
        Err(error) => {
            for diagnostic in error.diagnostics limit Iterations(32) {
                match diagnostic.location {
                    Some(location) => {
                        return diagnostic.severity == "error"
                            && diagnostic.code != ""
                            && diagnostic.message != ""
                            && location.source != ""
                            && location.revision != ""
                            && location.byte_start <= location.byte_end
                            && location.byte_end <= 7;
                    },
                    None => { return false; },
                }
            }
            return false;
        },
    }
}
"#,
        RunOptions::default(),
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_syntax_diagnostic_locations_preserve_the_native_source_identity_and_offsets() {
    use etas_code::syntax::{CodeSyntaxOperations, ParseBodyFailure};
    let backend = etas_frontend::code::service::NativeCodeSyntaxOperations::default();
    let Err(ParseBodyFailure::InvalidSyntax(expected)) = backend.parse_body("let = ;") else {
        panic!("invalid body must have parser diagnostics")
    };
    let location = expected.diagnostics[0].location.as_ref().unwrap();
    let result = run(r#"
module app.main;
import std.compiler.parse_body;
flow main() -> (string, string, usize, usize) {
    match parse_body("let = ;") {
        Err(error) => {
            for diagnostic in error.diagnostics limit Iterations(32) {
                match diagnostic.location {
                    Some(location) => { return (location.source, location.revision, location.byte_start, location.byte_end); },
                    None => {},
                }
            }
        },
        Ok(_) => {},
    }
    return ("", "", 0, 0);
}
"#, RunOptions::default()).await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::Tuple(
            vec![
                InterpValue::String(location.source.to_string().into()),
                InterpValue::String(location.revision.to_string().into()),
                InterpValue::usize(location.byte_start),
                InterpValue::usize(location.byte_end),
            ]
            .into()
        ))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_syntax_resource_exhaustion_is_not_a_parse_error() {
    let mut options = RunOptions::default();
    options.syntax_limits.parse.max_source_bytes = 8;
    let result = run(
        r#"
module app.main;
import std.compiler.parse_body;
flow main() -> i32 {
    match parse_body("return nonexistent();") {
        Ok(_) => { return 1; },
        Err(_) => { return 2; },
    }
}
"#,
        options,
    )
    .await;
    assert!(result.value().is_none());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("SourceBytes limit 8")),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_syntax_calls_share_the_run_owned_retention_budget() {
    let mut options = RunOptions::default();
    options.syntax_limits.max_retained_objects = 1;
    let result = run(
        r#"
module app.main;
import std.compiler.parse_body;
flow main() -> i32 {
    let first = parse_body("return 1;");
    match parse_body("return 2;") {
        Ok(_) => { return 1; },
        Err(_) => { return 2; },
    }
}
"#,
        options,
    )
    .await;
    assert!(result.value().is_none());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("RetainedObjects limit 1")),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_syntax_format_and_text_enforce_the_current_output_limit() {
    for projection in ["format(syntax)", "syntax.text"] {
        let source = format!(
            r#"
module app.main;
import std.compiler.parse_body;
import std.compiler.format;
flow main() -> string {{
    match parse_body("return 42;") {{
        Ok(syntax) => {{ return {projection}; }},
        Err(_) => {{ return "unexpected parse error"; }},
    }}
}}
"#
        );
        let mut options = RunOptions::default();
        options.syntax_limits.max_formatted_bytes = 4;
        let result = run(&source, options).await;
        assert!(result.value().is_none());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("FormattedBytes limit 4")),
            "{:?}",
            result.diagnostics
        );
    }
}

struct SyntaxClock {
    origin: std::time::Instant,
    reads: std::sync::atomic::AtomicU64,
    stop_at: Option<u64>,
    control: std::sync::Mutex<Option<api::RunControl>>,
}

impl SyntaxClock {
    fn new(stop_at: Option<u64>) -> Self {
        Self {
            origin: std::time::Instant::now(),
            reads: Default::default(),
            stop_at,
            control: Default::default(),
        }
    }

    fn reads(&self) -> u64 {
        self.reads.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn budget(self: &std::sync::Arc<Self>, max_millis: u64) -> etas_host::ExecutionBudget {
        etas_host::ExecutionBudget::start_with_clock(
            etas_host::Budget {
                time: Some(etas_host::TimeBudget { max_millis }),
                ..Default::default()
            },
            self.clone(),
        )
    }
}

impl etas_host::MonotonicClock for SyntaxClock {
    fn now(&self) -> std::time::Instant {
        let read = self
            .reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.stop_at == Some(read) {
            self.control
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .stop(etas_host::execution::CancellationReason::Requested)
                .unwrap();
        }
        self.origin + std::time::Duration::from_millis(read)
    }
}

fn parse_budget_source() -> String {
    let body = serde_json::to_string(&"let value = 1;".repeat(256)).unwrap();
    format!(
        r#"
module app.main;
import std.compiler.parse_body;
flow main() -> bool {{
    match parse_body({body}) {{
        Ok(_) => {{ return true; }},
        Err(_) => {{ return false; }},
    }}
}}
"#
    )
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_syntax_expires_the_parent_deadline_inside_parse_not_as_parse_error() {
    let clock = std::sync::Arc::new(SyntaxClock::new(None));
    let result = run(
        &parse_budget_source(),
        RunOptions {
            host_context: api::HostExecutionContext {
                budget: clock.budget(1000),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await;
    assert!(
        clock.reads() >= 1001,
        "parsing must check the live parent budget during work"
    );
    assert!(
        result.value().is_none(),
        "the ParseError match must not catch budget exhaustion"
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("execution time budget")),
        "{:?}",
        result.diagnostics
    );
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_syntax_cancellation_during_parse_preserves_run_control_flow() {
    let checked = checked_project(&parse_budget_source());
    let clock = std::sync::Arc::new(SyntaxClock::new(Some(1000)));
    let host = FakeHost::new(HostServiceAvailability::default());
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions {
            host_context: api::HostExecutionContext {
                budget: clock.budget(1_000_000),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    *clock.control.lock().unwrap() = Some(invocation.control());
    let result = invocation.execute().await.unwrap();
    assert!(
        matches!(result.outcome, api::RunOutcome::Cancelled(_)),
        "{:?}",
        result.outcome
    );
    assert!(result.value().is_none());
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(clock.reads() >= 1001);
    assert_eq!(host.console_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_syntax_format_and_text_preserve_parent_deadline_and_cancellation() {
    use etas_code::syntax::CodeSyntaxOperations;

    for projection in ["format(syntax)", "syntax.text"] {
        let checked = checked_project(&format!(
            r#"
module app.main;
import std.compiler.SyntaxBlock;
import std.compiler.format;
flow main(syntax: SyntaxBlock) -> string {{ return {projection}; }}
"#
        ));
        let backend = std::sync::Arc::new(
            etas_frontend::code::service::NativeCodeSyntaxOperations::default(),
        );
        let syntax = backend.parse_body(&"let value = 1;".repeat(256)).unwrap();
        let ty = checked.types.known_std_types.code.syntax_block.unwrap();
        let argument =
            InterpValue::SyntaxBlock(crate::value::SyntaxBlockValue::new(ty, syntax, backend));
        for cancel in [false, true] {
            let clock = std::sync::Arc::new(SyntaxClock::new(cancel.then_some(1000)));
            let host = FakeHost::new(HostServiceAvailability::default());
            let invocation = Interpreter.create_run(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![argument.clone()],
                &host,
                RunOptions {
                    host_context: api::HostExecutionContext {
                        budget: clock.budget(if cancel { 1_000_000 } else { 1000 }),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
            *clock.control.lock().unwrap() = Some(invocation.control());
            let result = invocation.execute().await.unwrap();
            assert!(result.value().is_none(), "{projection}");
            assert!(clock.reads() >= 1001, "{projection}");
            if cancel {
                assert!(
                    matches!(result.outcome, api::RunOutcome::Cancelled(_)),
                    "{:?}",
                    result.outcome
                );
                assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
            } else {
                assert!(
                    result
                        .diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.message.contains("execution time budget")),
                    "{projection}: {:?}",
                    result.diagnostics
                );
            }
        }
    }
}

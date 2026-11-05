use super::super::{FakeHost, availability, checked_project_with_options};
use crate::{
    Interpreter,
    api::{EntryPoint, RunOptions},
};
use etas_effects::HostRequirementKind;
use etas_frontend::{OptimizationLevel, ProjectCompileOptions};
use etas_host::HostErrorCode;

const SOURCE: &str = r#"
module app.main;

import std.effects.Console;
import std.io.println;
import std.json.JsonValue;
import std.json.parse;
import std.json.stringify;

flow payload_text() -> string {
    return "{\"scenario\":\"cross_level_semantics\",\"revision\":7}";
}

flow decode_payload(text: string) -> JsonValue {
    return parse(text).unwrap();
}

flow cross_boundary(value: JsonValue) -> JsonValue {
    return parse(stringify(value).unwrap()).unwrap();
}

flow main() -> string ![Console, Error<IOError>] {
    println("before");
    let decoded = decode_payload(payload_text());
    let published = cross_boundary(decoded);
    println("after");
    return stringify(published).unwrap();
}
"#;

const ERROR_SOURCE: &str = r#"
module app.main;

import std.effects.Console;
import std.io.read_line;

flow main() -> Result<string, IOError> ![Console] {
    return read_line()?;
}
"#;

#[derive(Debug, PartialEq)]
struct Observation {
    outcome: String,
    diagnostics: String,
    stdout: String,
    stderr: String,
    console_calls: usize,
    effects: etas_effects::EffectSummary,
}

#[tokio::test(flavor = "current_thread")]
async fn observable_order_errors_effects_and_traces_are_equal_at_every_level() {
    let mut successful = Vec::new();
    let mut failed = Vec::new();

    for level in [
        OptimizationLevel::O0,
        OptimizationLevel::O1,
        OptimizationLevel::O2,
        OptimizationLevel::O3,
    ] {
        successful.push(observe(SOURCE, level, false, true).await);
        failed.push(observe(ERROR_SOURCE, level, true, false).await);
    }

    for observation in &successful[1..] {
        assert_eq!(observation, &successful[0]);
    }
    assert_eq!(successful[0].stdout, "before\nafter\n");
    assert_eq!(successful[0].console_calls, 2);
    assert!(successful[0].outcome.contains("cross_level_semantics"));

    for observation in &failed[1..] {
        assert_eq!(observation, &failed[0]);
    }
    assert_eq!(failed[0].stdout, "");
    assert_eq!(failed[0].console_calls, 1);
    assert!(
        failed[0].outcome.contains("ProviderUnavailable"),
        "{}",
        failed[0].outcome
    );
}

async fn observe(
    source: &str,
    level: OptimizationLevel,
    fail_console: bool,
    expect_rewrite: bool,
) -> Observation {
    let checked = checked_project_with_options(
        source,
        ProjectCompileOptions {
            optimization_level: level,
            ..Default::default()
        },
    );
    assert_eq!(checked.compile_options.optimization_level, level);
    if expect_rewrite {
        let report = checked
            .optimization_report
            .pass("FuseSerializationBoundary");
        if level.enables(OptimizationLevel::O1) {
            assert!(report.is_some_and(|report| report.rewritten > 0));
        } else {
            assert!(report.is_none());
        }
    }

    let entry = checked.entry.expect("checked entry");
    let effects = checked.effects.item_effects[&entry].clone();
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    if fail_console {
        host.fail_console(HostErrorCode::ProviderUnavailable, "console unavailable");
    }
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint { item: entry },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .expect("execution lifecycle infrastructure");

    Observation {
        outcome: format!("{:?}", result.outcome),
        diagnostics: format!("{:?}", result.diagnostics),
        stdout: host.stdout_text(),
        stderr: host.stderr_text(),
        console_calls: host.console_call_count(),
        effects,
    }
}

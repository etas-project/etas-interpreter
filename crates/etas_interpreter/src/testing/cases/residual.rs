use super::super::*;

mod approval;
mod checkpoint;
mod compiled;
mod generic;
mod lifecycle;
mod outcomes;

fn residual_checked(source: &str) -> etas_frontend::CheckedProject {
    let output = project::residual_project_output(source);
    assert!(
        output.diagnostics.is_empty(),
        "residual program should check: {:#?}",
        output.diagnostics
    );
    let checked = output.checked.expect("checked residual program");
    assert!(
        !checked.residual_plans().is_empty(),
        "program should require a residual plan"
    );
    checked
}

async fn run(checked: &etas_frontend::CheckedProject, host: &FakeHost) -> api::RunResult {
    run_with(checked, host, vec![]).await
}

async fn run_with(
    checked: &etas_frontend::CheckedProject,
    host: &FakeHost,
    args: Vec<value::InterpValue>,
) -> api::RunResult {
    Interpreter
        .run_checked(
            checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            args,
            host,
            RunOptions::default(),
        )
        .await
        .unwrap()
}

fn residual_violation(result: &api::RunResult) -> Option<&etas_core::Diagnostic> {
    result.diagnostics.iter().find(|diagnostic| {
        diagnostic.code
            == DiagnosticCode::Analysis(AnalysisDiagnosticCode::TraceSpecResidualViolation)
    })
}

const ORDERED_HANDLED: &str = r#"
module app.main;
effect Gate { action check() -> unit; action send() -> unit; }
spec Safe: trace = Gate.check >> Gate.send;
flow main() -> i32 ![] ~ Safe {
    var sent = 0;
    handle {
        perform Gate.check();
        perform Gate.send();
    } with {
        Gate.check() => { resume (); }
        Gate.send() => { sent = sent + 1; resume (); }
    };
    return sent;
}
"#;

#[tokio::test(flavor = "current_thread")]
async fn managed_construction_and_calls_use_the_same_ordered_residual_monitor() {
    let checked = residual_checked(
        r#"
module app.main;
effect Init { action value() -> i32; action done() -> unit; }
agent Counter = { n: i32 };
impl Counter {
    public flow get(self) -> i32 ![] { return self.n; }
}
spec Safe: trace =
    (Agentic.create<Counter> >> Agentic.call<Counter.get>)
    & (Init.value >> Agentic.call<Counter.get>)
    & (Agentic.call<Counter.get> >> Init.done);
flow main() -> i32 ![] ~ Safe {
    let counter = handle { Counter { n = perform Init.value() } } with {
        Init.value() => { resume 7; }
    };
    let value = counter.get();
    handle { perform Init.done(); } with { Init.done() => { resume (); } };
    return value;
}
"#,
    );
    let result = run(&checked, &FakeHost::new(availability(&[]))).await;
    assert_eq!(
        result.value(),
        Some(&value::InterpValue::i32(7)),
        "{:?}",
        result.diagnostics
    );
    assert!(residual_violation(&result).is_none());
    let actions = result
        .events
        .iter()
        .filter_map(|event| match event {
            WorkflowEvent::AgentBoundary { action, .. } => Some(action.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(actions, ["Agentic.create", "Agentic.call"]);
}

#[tokio::test(flavor = "current_thread")]
async fn handled_guard_outcome_discharges_the_residual_before_dispatch() {
    let checked = residual_checked(ORDERED_HANDLED);
    let host = FakeHost::new(availability(&[]));
    let result = run(&checked, &host).await;
    assert!(
        result.value() == Some(&value::InterpValue::i32(1)),
        "{:#?}",
        result.diagnostics
    );
    assert!(residual_violation(&result).is_none());
}

fn conditional_guard(flag: bool) -> String {
    format!(
        r#"
module app.main;
effect Gate {{ action check() -> unit; action send() -> unit; }}
spec Safe: trace = Gate.check >> Gate.send;
flow main() -> i32 ![] ~ Safe {{
    var sent = 0;
    let approved = {flag};
    handle {{
        if approved {{
            perform Gate.check();
        }}
        perform Gate.send();
    }} with {{
        Gate.check() => {{ resume (); }}
        Gate.send() => {{ sent = sent + 1; resume (); }}
    }};
    return sent;
}}
"#
    )
}

#[test]
fn a_path_without_the_guard_is_a_static_violation_not_a_residual() {
    let output = project::residual_project_output(&conditional_guard(false));
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(
                etas_core::EffectDiagnosticCode::TraceSpecRequirementNotDominating
            )),
        "{:#?}",
        output.diagnostics
    );
}

const FAILED_GUARD_THEN_SEND: &str = r#"
module app.main;
effect Gate { action check() -> unit; action send() -> unit; }
spec Safe: trace = Gate.check >> Gate.send;
flow main() -> i32 ![] ~ Safe {
    var sent = 0;
    let guarded: Result<unit, i32> = {
        handle {
            perform Gate.check();
        } with {
            Gate.check() => { finish perform Error<i32>.raise(7); }
        }
    }?;
    handle {
        perform Gate.send();
    } with {
        Gate.send() => { sent = sent + 1; resume (); }
    };
    return sent;
}
"#;

#[tokio::test(flavor = "current_thread")]
async fn a_guard_that_failed_never_authorizes_its_target() {
    let output = project::residual_project_output(FAILED_GUARD_THEN_SEND);
    let checked = output
        .checked
        .unwrap_or_else(|| panic!("{:#?}", output.diagnostics));
    assert!(!checked.residual_plans().is_empty());
    let host = FakeHost::new(availability(&[]));
    let result = run(&checked, &host).await;
    assert!(result.value().is_none(), "{:#?}", result.value());
    let violation = residual_violation(&result)
        .unwrap_or_else(|| panic!("expected a residual violation: {:#?}", result.diagnostics));
    assert!(
        violation.message.contains("Gate.send"),
        "{}",
        violation.message
    );
}

use super::*;

pub(super) const HOST_GUARD: &str = r#"
module app.main;
import std.effects.Console;
import std.io.print;
effect Gate { action send() -> unit; }
spec Safe: trace = Console.stdout_write >> Gate.send;
flow main() -> i32 ![Console.stdout_write, Error<IOError>] ~ Safe {
    var sent = 0;
    print("guard");
    handle { perform Gate.send(); } with {
        Gate.send() => { sent = sent + 1; resume (); }
    };
    return sent;
}
"#;

#[tokio::test(flavor = "current_thread")]
async fn completed_host_guard_authorizes_the_target_once() {
    let checked = residual_checked(HOST_GUARD);
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let result = run(&checked, &host).await;
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(1)));
    assert_eq!(host.console_call_count(), 1);
    assert_eq!(host.stdout_text(), "guard");
}

#[tokio::test(flavor = "current_thread")]
async fn host_guard_outcome_survives_a_nested_call_boundary() {
    let source = HOST_GUARD
        .replace("flow main()", "flow guard() -> unit ![Console.stdout_write, Error<IOError>] { print(\"guard\"); }\nflow main()")
        .replace("    print(\"guard\");", "    guard();");
    let checked = residual_checked(&source);
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let result = run(&checked, &host).await;
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(1)));
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn host_guard_attempt_without_a_completed_outcome_never_authorizes_the_target() {
    let source = HOST_GUARD.replace(
        "    print(\"guard\");",
        r#"
        let ignored = {
            handle { perform Console.stdout_write("guard"); } with {
                Console.stdout_write(_) => { finish perform Error<i32>.raise(7); }
            }
        }?;
    "#,
    );
    let checked = residual_checked(&source);
    let host = FakeHost::new(availability(&[]));
    let result = run(&checked, &host).await;
    assert!(result.value().is_none());
    assert!(
        residual_violation(&result).is_some(),
        "{:#?}",
        result.diagnostics
    );
    assert_eq!(host.console_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_captured_host_failure_never_becomes_accepted_guard_evidence() {
    let source = HOST_GUARD.replace(
        "    print(\"guard\");",
        "    let ignored = print(\"guard\")?;",
    );
    let checked = residual_checked(&source);
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    host.fail_console(HostErrorCode::ProviderUnavailable, "test host failure");
    let result = run(&checked, &host).await;
    assert!(result.value().is_none());
    assert!(
        residual_violation(&result).is_some(),
        "{:#?}",
        result.diagnostics
    );
    assert_eq!(host.console_call_count(), 1);
    assert!(host.stdout_text().is_empty());
}

#[test]
fn a_source_effect_cannot_impersonate_the_standard_approval_identity() {
    let source = r#"
module app.main;
import std.effects.Console;
effect Approval { action request(reason: string) -> bool; }
spec Safe: trace = Approval.request >> Console.stdout_write;
flow main() -> unit ![Console.stdout_write] ~ Safe {
    let approved = handle { perform Approval.request("send the report") } with {
        Approval.request(_) => { resume true; }
    };
    perform Console.stdout_write("sent");
}
"#;
    let output = project::residual_project_output(source);
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(etas_core::EffectDiagnosticCode::IncompleteEffectFacts)),
        "{:#?}",
        output.diagnostics
    );
}

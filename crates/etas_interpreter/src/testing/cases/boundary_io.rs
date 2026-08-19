use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_missing_console_host_handler_for_std_io_entry() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.{println};

flow main() -> unit ![Console, Error<IOError>]
{
    println("hi");
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingHostHandler)
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_applies_policy_approval_before_console_boundary() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.println;

flow main() -> unit ![Console, Error<IOError>]
{
    println("hi");
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Approval,
    ]));
    let policy_ref = HostValue::String("console-policy".to_owned());
    host.seed_policy_decision(PolicyDecision::RequireApproval {
        request: ApprovalRequest {
            id: HostRequestId(900),
            reason: "console requires approval".to_owned(),
            requested_grants: Vec::new(),
            trace: TraceContext::root(TraceId(77)),
        },
    });

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: Vec::new(),
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: boundary_policy_context(policy_ref.clone()),
                    },
                    trace: TraceContext::root(TraceId(77)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.policy_call_count(), 1);
    assert_eq!(host.approval_call_count(), 1);
    assert_eq!(host.console_call_count(), 1);
    assert_eq!(host.stdout_text(), "hi\n");
    let requests = host.policy_requests();
    assert_eq!(requests[0].policy_ref, policy_ref);
    assert_eq!(requests[0].subject.kind, "console");
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::ApprovalRequested { request })
            if request.id == HostRequestId(900)
                && request.trace == TraceContext::root(TraceId(77))
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn policy_approval_grant_is_preserved_for_later_boundaries() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.println;

flow main() -> unit ![Console, Error<IOError>]
{
    println("first");
    println("second");
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Approval,
    ]));
    let policy_ref = HostValue::String("console-policy".to_owned());
    let approved_grant = HostActionGrant::allow("Console", "stdout_write");
    host.seed_policy_decision(PolicyDecision::RequireApproval {
        request: ApprovalRequest {
            id: HostRequestId(901),
            reason: "console requires approval".to_owned(),
            requested_grants: vec![approved_grant.clone()],
            trace: TraceContext::root(TraceId(88)),
        },
    });
    host.seed_policy_decision(PolicyDecision::Allow);

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: Vec::new(),
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: boundary_policy_context(policy_ref.clone()),
                    },
                    trace: TraceContext::root(TraceId(88)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.policy_call_count(), 2);
    assert_eq!(host.approval_call_count(), 1);
    let requests = host.policy_requests();
    assert!(
        requests[1].authority.grants.contains(&approved_grant),
        "later policy evaluations must see approval grants accepted earlier: {requests:#?}"
    );
    assert_eq!(host.stdout_text(), "first\nsecond\n");
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_restores_policy_approval_grants() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.println;
import std.runtime.{checkpoint};

flow main() -> unit ![Console, Error<IOError>]
{
    checkpoint("before");
    println("first");
    checkpoint("after-first");
    println("second");
}
"#,
    );

    let approved_grant = HostActionGrant::allow("Console", "stdout_write");
    let policy_ref = HostValue::String("console-policy".to_owned());
    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Approval,
        HostRequirementKind::Checkpoint,
    ]));
    first_host.seed_policy_decision(PolicyDecision::RequireApproval {
        request: ApprovalRequest {
            id: HostRequestId(902),
            reason: "console requires approval".to_owned(),
            requested_grants: vec![approved_grant.clone()],
            trace: TraceContext::root(TraceId(89)),
        },
    });
    first_host.seed_policy_decision(PolicyDecision::Allow);

    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: Vec::new(),
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: boundary_policy_context(policy_ref.clone()),
                    },
                    trace: TraceContext::root(TraceId(89)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.checkpoints.len(), 2);
    assert!(
        first.checkpoints[1]
            .host_context
            .authority
            .grants
            .contains(&approved_grant),
        "checkpoint should persist approval grants accepted before it"
    );

    let replay_host = FakeHost::new(availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Approval,
        HostRequirementKind::Checkpoint,
    ]));
    replay_host.seed_policy_decision(PolicyDecision::Allow);
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &first.checkpoints[1],
            &replay_host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: Vec::new(),
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: boundary_policy_context(policy_ref.clone()),
                    },
                    trace: TraceContext::root(TraceId(90)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    let replay_requests = replay_host.policy_requests();
    assert_eq!(replay_requests.len(), 1);
    assert!(
        replay_requests[0]
            .authority
            .grants
            .contains(&approved_grant),
        "resumed policy evaluation must receive grants restored from the checkpoint: {replay_requests:#?}"
    );
    assert_eq!(replay_host.stdout_text(), "second\n");
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_std_io_without_checked_action_mediation_fact() {
    let mut checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.println;

flow main() -> unit ![Console, Error<IOError>]
{
    println("hi");
}
"#,
    );
    let entry = checked.entry.expect("entry item");
    let summary = checked
        .effects
        .item_effects
        .get_mut(&entry)
        .expect("entry effect summary");
    summary.requested_actions = EffectRow::default();
    summary.default_actions = EffectRow::default();

    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint { item: entry },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingCheckedFact)
            && diagnostic
                .message
                .contains("checked default action facts for Console")
    }));
    assert!(
        host.console_call_count() == 0,
        "std.io must not reach the host without checked action mediation facts"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_std_command_without_checked_action_mediation_fact() {
    let mut checked = checked_project(
        r#"
module app.main;
import std.host.command.{Command, CommandResult, run};

flow main(cmd: Command) -> CommandResult ![Command.run<DefaultCommandSandbox>]
{
    return run(cmd, DefaultCommandSandbox);
}
"#,
    );
    let entry = checked.entry.expect("entry item");
    let summary = checked
        .effects
        .item_effects
        .get_mut(&entry)
        .expect("entry effect summary");
    summary.requested_actions = EffectRow::default();
    summary.default_actions = EffectRow::default();

    let host = FakeHost::new(availability(&[HostRequirementKind::Command]));
    host.seed_command_output(0, b"ok", b"");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint { item: entry },
            vec![value::InterpValue::Command {
                argv: vec!["echo".to_owned(), "ok".to_owned()],
                env: Vec::new(),
                cwd: None,
                stdin: None,
            }],
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingCheckedFact)
            && diagnostic
                .message
                .contains("checked default action facts for Command.run")
    }));
    assert_eq!(
        host.command_call_count(),
        0,
        "std.host.command.run must not reach the host without checked action mediation facts"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_std_command_through_command_host_service() {
    let checked = checked_project(
        r#"
module app.main;
import std.host.command.{Command, CommandResult, run};

flow main(cmd: Command) -> CommandResult ![Command.run<DefaultCommandSandbox>]
{
    return run(cmd, DefaultCommandSandbox);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Command]));
    host.seed_command_output(7, b"done", b"warn");
    let options = RunOptions {
        host_context: api::HostExecutionContext {
            authority: AuthorityContext {
                grants: vec![HostActionGrant::allow("Command", "run")],
                approvals: Vec::new(),
                sandbox: SandboxPolicy::deny_all(),
                policy: Default::default(),
            },
            trace: TraceContext::root(TraceId(77)),
            budget: etas_host::ExecutionBudget::default(),
        },
        ..RunOptions::default()
    };
    let command = value::InterpValue::Command {
        argv: vec!["echo".to_owned(), "done".to_owned()],
        env: vec![("LANG".to_owned(), "C".to_owned())],
        cwd: None,
        stdin: Some(b"input".to_vec()),
    };
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![command],
            &host,
            options,
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::CommandResult {
            exit_code: 7,
            stdout: b"done".to_vec(),
            stderr: b"warn".to_vec(),
        })
    );
    assert_eq!(host.command_call_count(), 1);
    let requests = host.command_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].argv, vec!["echo", "done"]);
    assert_eq!(requests[0].env, vec![("LANG".to_owned(), "C".to_owned())]);
    assert_eq!(requests[0].stdin, Some(b"input".to_vec()));
    assert_eq!(
        requests[0].authority.grants,
        vec![HostActionGrant::allow("Command", "run")]
    );
    assert_eq!(requests[0].trace, TraceContext::root(TraceId(77)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_applies_policy_deny_before_command_boundary() {
    let checked = checked_project(
        r#"
module app.main;
import std.host.command.{Command, CommandResult, run};

flow main(cmd: Command) -> CommandResult ![Command.run<DefaultCommandSandbox>]
{
    return run(cmd, DefaultCommandSandbox);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Command]));
    host.seed_command_output(0, b"should-not-run", b"");
    host.seed_policy_decision(PolicyDecision::Deny {
        reason: "command denied".to_owned(),
    });
    let policy_ref = HostValue::String("command-policy".to_owned());
    let command = value::InterpValue::Command {
        argv: vec!["echo".to_owned(), "blocked".to_owned()],
        env: Vec::new(),
        cwd: None,
        stdin: None,
    };

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![command],
            &host,
            RunOptions {
                host_context: api::HostExecutionContext {
                    authority: AuthorityContext {
                        grants: vec![HostActionGrant::allow("Command", "run")],
                        approvals: Vec::new(),
                        sandbox: SandboxPolicy::deny_all(),
                        policy: boundary_policy_context(policy_ref.clone()),
                    },
                    trace: TraceContext::root(TraceId(79)),
                    budget: etas_host::ExecutionBudget::default(),
                },
                ..RunOptions::default()
            },
        )
        .await;

    assert_eq!(result.value, None);
    assert_eq!(host.policy_call_count(), 1);
    assert_eq!(host.command_call_count(), 0);
    let requests = host.policy_requests();
    assert_eq!(requests[0].policy_ref, policy_ref);
    assert_eq!(requests[0].subject.kind, "command");
    assert!(
        requests[0].subject.attributes.iter().any(
            |(name, value)| name == "program" && value == &HostValue::String("echo".to_owned())
        )
    );
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("command policy denied request: command denied")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_std_io_through_console_host_service() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.{read_all, println};

flow main() -> unit ![Console, Error<IOError>]
{
    let input = read_all();
    println(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    host.seed_stdin("hello");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::Unit));
    assert_eq!(host.console_call_count(), 2);
    assert_eq!(host.stdout_text(), "hello\n");
    assert_eq!(host.stderr_text(), "");
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_captures_console_host_failure_with_try_expr() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.read_line;

flow main() -> Result<string, IOError> ![Console]

{
    return read_line()?;
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    host.fail_console(HostErrorCode::ProviderUnavailable, "stdin closed");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::Variant {
            name: "Err".to_owned(),
            fields: vec![value::InterpValue::Variant {
                name: "Host".to_owned(),
                fields: vec![value::InterpValue::String(
                    "ProviderUnavailable: stdin closed".to_owned()
                )],
            }],
        })
    );
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestFinished {
            outcome: etas_host::HostOutcome::Failed(error),
            ..
        }) if error.code == HostErrorCode::ProviderUnavailable
            && error.message == "stdin closed"
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_fails_closed_for_non_executable_checked_error_conversion() {
    let mut checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.read_line;

flow main() -> Result<string, IOError> ![Console]

{
    return read_line()?;
}
"#,
    );
    let io_error = resolve_std_type(&checked, &["std", "io", "IOError"])
        .expect("checked std.io program should contain IOError");
    let converted_error = checked.type_store.intern(Type::Named(NamedTypeRef {
        name: "ConvertedIOError".to_owned(),
    }));
    let capture = checked
        .effects
        .try_captures
        .values_mut()
        .next()
        .expect("read_line()? should have a checked capture fact");
    capture.captured_error = converted_error;
    capture.conversions = vec![ErrorConversionFact {
        source_error: io_error,
        target_error: converted_error,
    }];

    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    host.fail_console(HostErrorCode::ProviderUnavailable, "stdin closed");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert_eq!(host.console_call_count(), 1);
    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == DiagnosticCode::Analysis(AnalysisDiagnosticCode::UnhandledRuntimeError)
        }),
        "{:?}",
        result.diagnostics
    );
    assert_eq!(result.value, None);
    assert_eq!(
        result.diagnostics.len(),
        1,
        "terminal error conversion failure must have one diagnostic owner"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_try_expr_without_checked_capture_fact() {
    let mut checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.read_line;

flow main() -> Result<string, IOError> ![Console]

{
    return read_line()?;
}
"#,
    );
    checked.effects.try_captures.clear();

    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingCheckedFact)
    }));
    assert_eq!(
        host.console_call_count(),
        0,
        "interpreter must not cross host boundary when checked TryCaptureFact is missing"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_perform_without_checked_action_fact() {
    let mut checked = checked_project(
        r#"
module app.main;

effect Approval {
    action request(reason: string) -> bool;
}

flow main() -> unit ![Approval] {
    perform Approval.request("ship");
    return;
}
"#,
    );
    checked.effects.performed_actions.clear();

    let host = FakeHost::new(availability(&[HostRequirementKind::Approval]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingCheckedFact)
    }));
    assert_eq!(
        host.approval_call_count(),
        0,
        "interpreter must not cross effect boundary when checked PerformedActionFact is missing"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resumes_console_in_var_initializer() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.{read_line, println};

flow main() -> unit ![Console, Error<IOError>]
{
    var input: string = read_line();
    println(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    host.seed_stdin("var-value\n");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.stdout_text(), "var-value\n\n");
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resumes_console_in_assign_rhs() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.{read_line, println};

flow main() -> unit ![Console, Error<IOError>]
{
    var input: string = "";
    input = read_line();
    println(input);
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    host.seed_stdin("assigned\n");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(host.stdout_text(), "assigned\n\n");
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_reports_orchestration_gap_for_resume_flow() {
    let mut checked = checked_project(
        r#"
module app.main;

flow main() -> unit {
  return;
}
"#,
    );
    let entry = checked.entry.expect("entry item");
    checked.interpreter_support.entry = Some(InterpreterSupport::RequiresInterpreterOrchestration(
        etas_effects::InterpreterOrchestrationRequirementSet::feature(
            etas_effects::InterpreterFeatureKind::Resume,
        ),
    ));
    checked
        .interpreter_support
        .items
        .insert(entry, checked.interpreter_support.entry.clone().unwrap());

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint { item: entry },
            Vec::new(),
            &FakeHost::new(availability(&[HostRequirementKind::Network])),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code
            == DiagnosticCode::Analysis(AnalysisDiagnosticCode::UnsupportedPhase2RuntimeFeature)
    }));
}

use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn run_checked_records_checkpoint_event_and_snapshot() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

flow main() -> unit {
  checkpoint("before-review");
  return;
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
            &FakeHost::new(availability(&[HostRequirementKind::Checkpoint])),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let checkpoint = result.checkpoints.first().expect("checkpoint snapshot");
    assert!(result.events.iter().any(
        |event| matches!(event, WorkflowEvent::CheckpointCreated(id) if *id == checkpoint.id)
    ));
    assert_eq!(result.checkpoints.len(), 1);
    assert_eq!(checkpoint.label.as_deref(), Some("before-review"));
    assert_eq!(checkpoint.entry_item, checked.entry.expect("entry item"));
    assert!(!checkpoint.machine.frames.is_empty());
    assert!(checkpoint.handlers.handlers.is_empty());
    assert!(checkpoint.retry_state.attempts.is_empty());
    assert!(checkpoint.trace.events_recorded >= 2);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_handler_resume_for_let_bound_perform() {
    let checked = checked_project(
        r#"
module app.main;

effect Approval {
  action request(value: string) -> string;
}

flow main(input: string) -> string {
  let result = handle {
    let approved = perform Approval.request(input);
    approved
  } with {
    Approval.request(value) => {
      resume value;
    }
  };

  return result;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("approved".to_owned())],
            &FakeHost::new(availability(&[HostRequirementKind::Approval])),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("approved".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_handler_finish_for_never_action() {
    let checked = checked_project(
        r#"
module app.main;

effect Abort {
  action stop() -> never;
}

flow main() -> string {
  return handle {
    perform Abort.stop();
    "after"
  } with {
    Abort.stop() => {
      finish "fallback";
    }
  };
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
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
        Some(value::InterpValue::String("fallback".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_top_level_handler_value() {
    let checked = checked_project(
        r#"
module app.main;

effect Approval {
  action request(message: string) -> bool;
}

let AutoApproval: ![Approval => []] = handler {
  Approval.request(message) => {
    resume true;
  }
};

flow main() -> bool {
  return handle perform Approval.request("ship") with AutoApproval;
}
"#,
    );

    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
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
    assert_eq!(result.value, Some(value::InterpValue::Bool(true)));
    assert_eq!(host.approval_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_first_class_handler_value_through_call() {
    let checked = checked_project(
        r#"
module app.main;

effect Approval {
  action request(message: string) -> bool;
}

let AutoApproval: ![Approval => []] = handler {
  Approval.request(message) => {
    resume true;
  }
};

flow choose(approval_handler: ![Approval => []]) -> ![Approval => []] {
  return approval_handler;
}

flow main() -> bool {
  let selected = choose(AutoApproval);
  return handle perform Approval.request("ship") with selected;
}
"#,
    );

    let host = FakeHost::new(HostServiceAvailability::default());
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
    assert_eq!(result.value, Some(value::InterpValue::Bool(true)));
    assert_eq!(host.approval_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_handle_without_checked_application_fact() {
    let mut checked = checked_project(
        r#"
module app.main;

effect Approval {
  action request(value: string) -> string;
}

flow main(input: string) -> string {
  let result = handle {
    let approved = perform Approval.request(input);
    approved
  } with {
    Approval.request(value) => {
      resume value;
    }
  };

  return result;
}
"#,
    );
    checked.effects.handle_applications.clear();

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("approved".to_owned())],
            &FakeHost::new(availability(&[HostRequirementKind::Approval])),
            RunOptions::default(),
        )
        .await;

    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingCheckedFact)
        }),
        "{:?}",
        result.diagnostics
    );
    assert_eq!(result.value, None);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_rejects_handle_without_checked_handler_value_fact() {
    let mut checked = checked_project(
        r#"
module app.main;

effect Approval {
  action request(value: string) -> string;
}

flow main(input: string) -> string {
  let result = handle {
    let approved = perform Approval.request(input);
    approved
  } with {
    Approval.request(value) => {
      resume value;
    }
  };

  return result;
}
"#,
    );
    checked.effects.handler_values.clear();

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("approved".to_owned())],
            &FakeHost::new(availability(&[HostRequirementKind::Approval])),
            RunOptions::default(),
        )
        .await;

    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingCheckedFact)
        }),
        "{:?}",
        result.diagnostics
    );
    assert_eq!(result.value, None);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_resumes_console_value_inside_handler_resume() {
    let checked = checked_project(
        r#"
module app.main;

import std.effects.Console;
import std.io.read_line;

effect Approval {
  action request(value: string) -> string;
}

flow main() -> string ![Console.stdin_read_line, Error<IOError>, Approval.request]
{
  let result = handle {
    perform Approval.request("ignored")
  } with {
    Approval.request(_) => {
      resume read_line();
    }
  };

  return result;
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Approval,
        HostRequirementKind::Console,
    ]));
    host.seed_stdin("resumed\n");
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
        Some(value::InterpValue::String("resumed\n".to_owned()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_nested_perform_inside_handler_body() {
    let checked = checked_project(
        r#"
module app.main;

effect Approval {
  action decide(reason: string) -> bool;
  action request(reason: string) -> bool;
}

flow main() -> bool {
  let result = handle {
    perform Approval.decide("Ship this?")
  } with {
    Approval.decide(reason) => {
      let approved = perform Approval.request(reason);
      resume approved;
    }
  };

  return result;
}
"#,
    );

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

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::Bool(true)));
    assert_eq!(host.approval_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_approval_host_boundary_and_records_ledger() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

effect Approval {
  action request(reason: string) -> bool;
}

flow main() -> bool {
  let approved = perform Approval.request("Ship this?");
  checkpoint("after-approval");
  return approved;
}
"#,
    );

    let host = FakeHost::new(availability(&[
        HostRequirementKind::Approval,
        HostRequirementKind::Checkpoint,
    ]));

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
    assert_eq!(result.value, Some(value::InterpValue::Bool(true)));
    assert_eq!(host.approval_call_count(), 1);
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted { .. })
    )));
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestFinished { .. })
    )));
    let checkpoint = result.checkpoints.first().expect("checkpoint record");
    assert_eq!(checkpoint.completed_host_boundaries.completed.len(), 1);
    assert_eq!(
        checkpoint.completed_host_boundaries.completed[0].kind,
        "approval"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_approval_host_boundary_inside_helper_flow() {
    let checked = checked_project(
        r#"
module app.main;

effect Approval {
  action request(reason: string) -> bool;
}

flow helper(reason: string) -> bool {
  return perform Approval.request(reason);
}

flow main() -> bool {
  return helper("Ship this?");
}
"#,
    );

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

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::Bool(true)));
    assert_eq!(host.approval_call_count(), 1);
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted { .. })
    )));
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestFinished { .. })
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_preserves_caller_continuation_after_helper_flow_perform() {
    let checked = checked_project(
        r#"
module app.main;

effect Approval {
  action request(reason: string) -> bool;
}

flow helper(reason: string) -> bool {
  return perform Approval.request(reason);
}

flow main() -> bool {
  let approved = helper("Ship this?");
  return approved;
}
"#,
    );

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

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::Bool(true)));
    assert_eq!(host.approval_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_approval_host_boundary_inside_call_argument() {
    let checked = checked_project(
        r#"
module app.main;

effect Approval {
  action request(reason: string) -> bool;
}

flow identity(value: bool) -> bool {
  return value;
}

flow main() -> bool {
  return identity(perform Approval.request("Ship this?"));
}
"#,
    );

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

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::Bool(true)));
    assert_eq!(host.approval_call_count(), 1);
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted { .. })
    )));
    assert!(result.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestFinished { .. })
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_executes_host_boundary_inside_perform_payload_argument() {
    let checked = checked_project(
        r#"
module app.main;

effect Approval {
  action request(reason: string) -> bool;
}

flow helper() -> string {
  let _approved = perform Approval.request("inner");
  return "outer";
}

flow main() -> bool {
  return perform Approval.request(helper());
}
"#,
    );

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

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::Bool(true)));
    assert_eq!(host.approval_call_count(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_propagates_return_from_call_argument_evaluation() {
    let mut checked = checked_project(
        r#"
module app.main;

flow identity(value: bool) -> bool {
  return value;
}

flow main() -> bool {
  return identity(true);
}
"#,
    );

    let entry = checked.entry.expect("entry item");
    let HirItem::Flow(main_flow) = checked.hir.items.get(entry).expect("entry flow") else {
        panic!("expected flow entry");
    };
    let main_body = main_flow.body.block();
    let return_stmt = checked.hir.blocks[main_body]
        .stmts
        .first()
        .copied()
        .expect("main return stmt");
    let HirStmt::Return {
        value: Some(call_expr),
        ..
    } = checked.hir.stmts[return_stmt]
    else {
        panic!("expected return call expr");
    };
    let call_span = checked.hir.exprs[call_expr].span(&checked.hir.blocks);
    let scope = checked.hir.blocks[main_body].scope;
    let HirExpr::Call { args, .. } = checked.hir.exprs.get(call_expr).expect("call expr") else {
        panic!("expected call expr");
    };
    let HirArg::Positional(original_arg) = args[0] else {
        panic!("expected positional call arg");
    };
    let return_stmt = checked.hir.stmts.alloc(HirStmt::Return {
        value: Some(original_arg),
        span: call_span,
    });
    let arg_block = checked.hir.blocks.alloc_with_id(|id| HirBlock {
        id,
        stmts: vec![return_stmt],
        final_expr: None,
        scope,
        span: call_span,
    });
    let arg_expr = checked.hir.exprs.alloc(HirExpr::Block(arg_block));
    let HirExpr::Call { args, .. } = checked.hir.exprs.get_mut(call_expr).expect("call expr")
    else {
        panic!("expected call expr");
    };
    args[0] = HirArg::Positional(arg_expr);

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint { item: entry },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_checked_propagates_return_from_perform_payload_argument_evaluation() {
    let mut checked = checked_project(
        r#"
module app.main;

effect Approval {
  action request(reason: string) -> bool;
}

flow main() -> string {
  perform Approval.request("outer");
  return "unreachable";
}
"#,
    );

    let entry = checked.entry.expect("entry item");
    let HirItem::Flow(main_flow) = checked.hir.items.get(entry).expect("entry flow") else {
        panic!("expected flow entry");
    };
    let main_body = main_flow.body.block();
    let perform_stmt = checked.hir.blocks[main_body]
        .stmts
        .first()
        .copied()
        .expect("perform stmt");
    let HirStmt::Expr {
        expr: perform_expr, ..
    } = checked.hir.stmts[perform_stmt]
    else {
        panic!("expected expr stmt");
    };
    let perform_span = checked.hir.exprs[perform_expr].span(&checked.hir.blocks);
    let scope = checked.hir.blocks[main_body].scope;
    let HirExpr::Perform { args, .. } = checked.hir.exprs.get(perform_expr).expect("perform expr")
    else {
        panic!("expected perform expr");
    };
    let HirArg::Positional(original_arg) = args[0] else {
        panic!("expected positional perform arg");
    };
    let return_stmt = checked.hir.stmts.alloc(HirStmt::Return {
        value: Some(original_arg),
        span: perform_span,
    });
    let arg_block = checked.hir.blocks.alloc_with_id(|id| HirBlock {
        id,
        stmts: vec![return_stmt],
        final_expr: None,
        scope,
        span: perform_span,
    });
    let arg_expr = checked.hir.exprs.alloc(HirExpr::Block(arg_block));
    let HirExpr::Perform { args, .. } = checked
        .hir
        .exprs
        .get_mut(perform_expr)
        .expect("perform expr")
    else {
        panic!("expected perform expr");
    };
    args[0] = HirArg::Positional(arg_expr);

    let host = FakeHost::new(availability(&[HostRequirementKind::Approval]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint { item: entry },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Some(value::InterpValue::String("outer".to_owned()))
    );
    assert_eq!(host.approval_call_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn resume_checkpoint_reuses_completed_approval_boundary_result() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};

effect Approval {
  action request(reason: string) -> bool;
}

flow main() -> bool {
  checkpoint("before-approval");
  let approved = perform Approval.request("Ship this?");
  checkpoint("after-approval");
  return approved;
}
"#,
    );

    let first_host = FakeHost::new(availability(&[
        HostRequirementKind::Approval,
        HostRequirementKind::Checkpoint,
    ]));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions::default(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first_host.approval_call_count(), 1);
    assert_eq!(first.checkpoints.len(), 2);

    let mut replay_checkpoint = first.checkpoints[0].clone();
    replay_checkpoint.completed_host_boundaries =
        first.checkpoints[1].completed_host_boundaries.clone();

    let replay_host = FakeHost::new(availability(&[
        HostRequirementKind::Approval,
        HostRequirementKind::Checkpoint,
    ]));
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &replay_checkpoint,
            &replay_host,
            RunOptions::default(),
        )
        .await;

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value, Some(value::InterpValue::Bool(true)));
    assert_eq!(replay_host.approval_call_count(), 0);
    assert!(!resumed.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestStarted { .. })
    )));
    assert!(!resumed.events.iter().any(|event| matches!(
        event,
        WorkflowEvent::HostTrace(etas_host::TraceEvent::HostRequestFinished { .. })
    )));
}

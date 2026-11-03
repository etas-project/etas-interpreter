use super::*;
use crate::api::RunOutcome;
use etas_host::execution::ExecutionScope;

mod privacy;

fn completed(value: InterpValue) -> RunResult {
    let scope = ExecutionScope::new_owned();
    scope.finish_body(true).unwrap();
    RunResult {
        outcome: RunOutcome::Completed(value),
        termination: scope.termination().unwrap().unwrap(),
        diagnostics: vec![],
        events: vec![],
        checkpoints: vec![],
    }
}

#[test]
fn report_ownership_and_output_are_stack_safe_for_deep_values() {
    const CHILD: &str = "ETAS_TEST_DEEP_RUN_REPORT";
    if std::env::var_os(CHILD).is_none() {
        let current = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let mut value = InterpValue::Bool(true);
    for _ in 0..30_000 {
        value = InterpValue::OptionSome(value.into());
    }
    let mut result = completed(value.clone());
    result.events.push(WorkflowEvent::MessageCreated {
        id: "deep-message".into(),
        from: None,
        to: None,
        session: None,
        role: "user".into(),
        created_at: "step-1".into(),
        payload: Box::new(value),
        provenance: None,
    });
    let report = run_report_json("run", &[], "main", &result).unwrap();
    let duplicate = report.clone();
    assert_eq!(report, duplicate);
    let text = format!("{report}");
    assert_eq!(text.matches("option_some").count(), 60_000);
    assert!(text.contains("deep-message"));
    assert!(text.contains("etas.cli.interpreter-report.v2"));
    assert!(text.ends_with('}'));
}

#[test]
fn late_event_error_releases_already_encoded_deep_report_without_swallowing_error() {
    use crate::control::{CallTarget, Frame};
    use etas_hir::{HirExprId, SymbolId};

    let mut frame = Frame::from_snapshot(vec![(SymbolId(0), InterpValue::Unit)]).unwrap();
    let invalid = InterpValue::Callable(CallTarget::Lambda {
        expr: HirExprId(7),
        captured: frame.clone(),
    });
    assert!(frame.set(SymbolId(0), invalid.clone()));
    let mut deep = InterpValue::Bool(true);
    for _ in 0..30_000 {
        deep = InterpValue::OptionSome(deep.into());
    }
    let event = |id: &str, payload| WorkflowEvent::MessageCreated {
        id: id.into(),
        from: None,
        to: None,
        session: None,
        role: "user".into(),
        created_at: "step-1".into(),
        payload: Box::new(payload),
        provenance: None,
    };
    let mut result = completed(deep.clone());
    result.events = vec![event("valid", deep), event("invalid", invalid)];
    let (_, cost) = crate::testing::allocation::measure(|| {
        let error = run_report_json("run", &[], "main", &result).unwrap_err();
        assert!(error.message().contains("cyclic captured-frame"), "{error}");
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "report leaked on error: {cost:?}"
    );
    // Release the intentionally malformed native cycle after testing its rejection.
    assert!(frame.set(SymbolId(0), InterpValue::Unit));
}

use super::*;
use crate::control::{CallTarget, Frame};
use crate::value::{RecordValue, StringValue};
use etas_hir::{HirExprId, SymbolId};
use etas_types::TrustWrapper;

fn secret(value: InterpValue) -> InterpValue {
    InterpValue::Trust {
        wrapper: TrustWrapper::Secret,
        value: value.into(),
    }
}

#[test]
fn reports_redact_secret_results_and_message_events_without_changing_values() {
    let marker = "AUDIT_ONLY_REPORT_SECRET_PAYLOAD";
    let payload = secret(InterpValue::Record(RecordValue::new(vec![(
        "text".into(),
        InterpValue::String(marker.into()),
    )])));
    let mut result = completed(InterpValue::OptionSome(payload.clone().into()));
    result.events.push(WorkflowEvent::MessageCreated {
        id: "message".into(),
        from: None,
        to: None,
        session: None,
        role: "user".into(),
        created_at: "step-1".into(),
        payload: Box::new(payload.clone()),
        provenance: None,
    });
    let report = run_report_json("run", &[], "main", &result).unwrap();
    assert!(!report.to_string().contains(marker));
    assert!(report.to_string().contains("redacted"));
    assert!(
        value_from_json(&report["value"]).is_err(),
        "a redacted observation is not a restorable value"
    );
    assert_eq!(
        result.value(),
        Some(&InterpValue::OptionSome(payload.into()))
    );
    assert!(
        !event_json(&result.events[0])
            .unwrap()
            .to_string()
            .contains(marker)
    );
}

#[test]
fn report_redaction_does_not_enter_secret_captured_frame_cycles() {
    let mut frame = Frame::from_snapshot(vec![(SymbolId(0), InterpValue::Unit)]).unwrap();
    let callable = InterpValue::Callable(CallTarget::Lambda {
        expr: HirExprId(7),
        captured: frame.clone(),
    });
    frame.set(SymbolId(0), secret(callable.clone()));
    let result = completed(secret(callable));
    let report = run_report_json("run", &[], "main", &result);
    frame.set(SymbolId(0), InterpValue::Unit);
    let report = report.unwrap();
    assert!(report.to_string().contains("redacted"));
    assert!(!report.to_string().contains("locals"));
}

#[tokio::test(flavor = "current_thread")]
async fn reports_only_export_checkpoint_observations_and_preserve_in_memory_resume() {
    let checked = crate::testing::project::checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow consume(value: Secret<string>) -> i32 { return 42; }
flow main() -> i32 {
    let value = Secret("AUDIT_ONLY_CHECKPOINT_REPORT_SECRET");
    checkpoint("secret-is-live");
    return consume(value);
}
"#,
    );
    let host = crate::testing::FakeHost::new(crate::host::HostServiceAvailability {
        checkpoint: true,
        ..Default::default()
    });
    let result = crate::Interpreter
        .run_checked(
            &checked,
            crate::api::EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            crate::api::RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.checkpoints.len(), 1);
    let report = run_report_json("run", &[], "main", &result).unwrap();
    assert!(
        !report
            .to_string()
            .contains("AUDIT_ONLY_CHECKPOINT_REPORT_SECRET")
    );
    let observation = &report["checkpoints"][0];
    assert_eq!(observation["kind"], "checkpoint_summary");
    assert_eq!(observation["resumable"], false);
    assert!(observation.get("machine").is_none());
    assert_eq!(observation["label"], "secret-is-live");
    let resumed = crate::Interpreter
        .resume_checkpoint(
            &checked,
            &result.checkpoints[0],
            &host,
            crate::api::RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&InterpValue::i32(42)));
}

#[test]
fn report_redaction_never_materializes_cold_secret_text() {
    use etas_builtin::text::input::TextInput;
    let prefix = StringValue::from("AUDIT_ONLY_COLD_SECRET".repeat(4000));
    let input = prefix.clone().concat("tail".into()).unwrap();
    let result = completed(secret(InterpValue::String(input.clone())));
    let (report, cost) = crate::testing::allocation::measure(|| {
        run_report_json("run", &[], "main", &result).unwrap()
    });
    assert!(!report.to_string().contains("AUDIT_ONLY_COLD_SECRET"));
    assert!(input.contiguous().is_none());
    assert!(
        cost.bytes < 16_384,
        "redaction copied the secret first: {cost:?}"
    );
}

use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn explicit_effect_rows_and_generic_closure_captures_survive_checkpoint_restore() {
    let checked = checked_project(
        r#"module app.main;
import std.runtime.checkpoint;
flow make<T, effect E>(f: (T) -> T ![E]) -> ((T) -> T ![E]) ![] {
    return (value: T) => { return f(value); };
}
flow main() -> i32 {
    let saved = make<i32, ![]>((value: i32) => { return value; });
    checkpoint("generic-capture");
    return saved(42);
}
"#,
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
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(42)));
    assert_eq!(result.checkpoints.len(), 1);
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &decoded, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), result.value());
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_effect_row_call_rejects_missing_checked_instantiation() {
    let mut checked = checked_project(
        r#"module app.main;
flow make<T, effect E>(f: (T) -> T ![E]) -> ((T) -> T ![E]) ![] {
    return (value: T) => { return f(value); };
}
flow main() -> i32 ![] {
    let saved = make<i32, ![]>((value: i32) => { return value; });
    return saved(42);
}
"#,
    );
    let calls = checked
        .types
        .generic_instantiations
        .iter()
        .filter(|(_, fact)| !fact.effect_row_bindings.is_empty())
        .map(|(call, _)| *call)
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 1);
    let facts = std::sync::Arc::make_mut(&mut std::sync::Arc::make_mut(&mut checked).types);
    for call in calls {
        facts.generic_instantiations.remove(&call);
    }
    let host = FakeHost::new(HostServiceAvailability::default());
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.value().is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Analysis(AnalysisDiagnosticCode::MissingCheckedFact)),
        "{:?}",
        result.diagnostics
    );
}

use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn empty_tuple_executes_as_the_unit_value() {
    let checked = checked_project("module app.main; flow main() -> () { return (); }");
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::Unit));
}

#[tokio::test(flavor = "current_thread")]
async fn unit_patterns_and_generic_values_survive_checkpoint_replay() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow identity(value: ()) -> unit { return value; }
flow main() -> i32 {
    let pair: (unit, i32) = (identity(()), 7);
    let optional: Option<unit> = Some(());
    let result: Result<(), string> = Ok(());
    let extracted: unit = std.option.unwrap(optional);
    checkpoint("unit-values");
    let ((), number) = pair;
    let first = match optional { Some(()) => number, None => 0 };
    let second = match result { Ok(()) => 3, Err(_) => 0 };
    match extracted { () => {} };
    return match () { () => first + second };
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
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(10)));
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), result.value());

    let mut corrupt = checkpoint.clone();
    corrupt
        .args
        .push(crate::orchestration::ValueSnapshot::Tuple(
            Vec::new().into(),
        ));
    let rejected = Interpreter
        .resume_checkpoint(&checked, &corrupt, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(rejected.value().is_none());
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("canonical unit")),
        "{:?}",
        rejected.diagnostics
    );
    let corrupt = checkpoint_artifact_json(&["main.es".into()], "main", &corrupt).unwrap();
    assert!(
        checkpoint_from_json(&corrupt, &checked)
            .unwrap_err()
            .message()
            .contains("canonical unit")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn inline_finish_result_constructors_use_the_handled_answer_type() {
    for (completion, expected) in [("Err(\"sentinel\")", 0), ("Ok(Value { number = 11 })", 11)] {
        let checked = checked_project(&format!(
            r#"
module app.main;
effect Probe {{ action ask() -> bool; }}
type Value = {{ number: i32 }};
flow controller() -> Result<Value, string> ![Probe] {{
    let selected = perform Probe.ask();
    if selected {{ Ok(Value {{ number = 7 }}) }} else {{ Err("controller") }}
}}
flow main() -> i32 {{
    let value = controller() with {{ Probe.ask() => {{ finish {completion}; }} }};
    return match value {{ Ok(value) => value.number, Err(_) => 0 }};
}}
"#
        ));
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                Vec::new(),
                &FakeHost::new(HostServiceAvailability::default()),
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&value::InterpValue::i32(expected)));
    }
}

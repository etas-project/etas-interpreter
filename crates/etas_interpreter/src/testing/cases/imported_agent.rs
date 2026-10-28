use super::super::*;
use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[tokio::test(flavor = "current_thread")]
async fn imported_agent_default_runs_in_the_implementation_module() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.sources = [
        "module app.component; public agent Worker;",
        "module app.implementation; import app.component.Worker as Target; flow adjust(value: i32) -> i32 { return value + 2; } impl agent Target { public flow run(value: i32) -> i32 { return adjust(value); } }",
        "module app.main; import app.component.Worker; flow main() -> i32 { return Worker(40); }",
    ].into_iter().enumerate().map(|(index, text)| SourceInput {
        id: etas_core::SourceId(index as u32 + 1), path: None, text: text.into(), kind: etas_frontend::SourceKind::SourceProjectFile,
    }).collect();
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
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
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&InterpValue::Number(value::NumericValue::i32(42)))
    );
    assert_eq!(host.model_call_count(), 0);
}

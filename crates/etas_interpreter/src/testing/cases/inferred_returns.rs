use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn inferred_agent_methods_execute_the_checked_output_without_model_calls() {
    let checked = checked_project(
        r#"
module app.main;
agent Worker;
impl agent Worker {
    flow run(value: i32) { return value + 1; }
    flow explain(value: i32) { return run(value) + 1; }
}
flow main() { return Worker(19) + Worker.explain(20); }
"#,
    );
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

#[tokio::test(flavor = "current_thread")]
async fn inferred_mutually_recursive_outputs_execute_using_finalized_types() {
    let checked = checked_project(
        r#"
module app.main;
flow even(n: i32) { if n == 0 { return true; } return odd(n - 1); }
flow odd(n: i32) { if n == 0 { return false; } return even(n - 1); }
flow main() { return even(10); }
"#,
    );
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
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

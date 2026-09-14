use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn checked_map_counts_and_unicode_indexing_execute_without_host_calls() {
    let checked = checked_project(
        r#"
module app.main;
import std.collections.len;
import std.collections.is_empty;
flow main(values: Map<string, string>) -> bool {
    if is_empty(values) { return len(values) == 0; }
    let text = "aé中🙂";
    return len(values) == 2
        && values["first"] == "payload"
        && text[0] == 'a'
        && text[1] == 'é'
        && text[2] == '中'
        && text[3] == '🙂';
}
"#,
    );
    for entries in [
        vec![],
        vec![
            (
                InterpValue::String("first".into()),
                InterpValue::String("payload".into()),
            ),
            (
                InterpValue::String("second".into()),
                InterpValue::String("unchanged".into()),
            ),
        ],
    ] {
        let input = InterpValue::Map(entries.into());
        let alias = input.clone();
        let host = FakeHost::new(HostServiceAvailability::default());
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![input.clone()],
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(input, alias);
    }
}

use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn checked_global_counts_preserve_suspension_unicode_and_sliced_views() {
    let checked = checked_project(
        r#"
module app.main;
import std.collections.len;
import std.collections.is_empty;
import std.io.println;
import std.runtime.checkpoint;
flow text() -> string {
    println("text")?;
    checkpoint("text");
    return "中😀é";
}
flow identity<T>(value: T) -> T { return value; }
flow main() -> bool {
    let array = [1, 2, 3, 4];
    let alias = array;
    let slice = array[1, 3);
    let empty = array[2, 2);
    let list = [1; 2; 3];
    let count = len(identity(text()));
    return count == 4 && len(array) == 4 && len(list) == 3
        && len(slice) == 2 && !is_empty(slice) && is_empty(empty)
        && len(empty) == 0 && len("") == 0 && is_empty("")
        && !is_empty("😀") && len("😀") == 1 && array == alias;
}
"#,
    );
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let host = FakeHost::new(services);
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
    assert_eq!(host.stdout_text(), "text\n");
    assert_eq!(result.checkpoints.len(), 1);
    let saved = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    for checkpoint in [saved, &decoded] {
        let host = FakeHost::new(services);
        let resumed = Interpreter
            .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(host.stdout_text(), "");
    }
}

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

use super::super::*;
use crate::api::codec::{
    CheckpointFileLimits, checkpoint_artifact_json, checkpoint_file_from_bytes,
    checkpoint_file_to_bytes, checkpoint_from_json,
};

#[tokio::test(flavor = "current_thread")]
async fn shared_text_aliases_and_queries_survive_nested_projection_and_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.text.contains;
import std.text.starts_with;
import std.text.ends_with;
import std.text.len;
import std.text.split;
import std.text.lines;
import std.text.join;
import std.text.trim;
import std.text.lowercase;
import std.text.uppercase;
import std.runtime.checkpoint;
type Text = string;
type Row = { text: string }
flow main() -> bool {
    var text = "中😀é";
    let alias = text;
    let row = Row { text = text };
    let wrapped = Some(Ok<string, string>(text));
    let unchanged = trim(alias);
    let whole_parts = split(alias, "absent");
    let shortened = trim("   small   ");
    text = text + "suffix";
    checkpoint("text-aliases");
    text = text + "!";
    let parts = split(alias, "😀");
    let joined = join(parts, "😀");
    return alias == "中😀é" && row.text == alias
        && wrapped == Some(Ok<string, string>(alias)) && joined == alias
        && len(alias) == 4 && contains(text, "suffix")
        && starts_with(text, alias) && ends_with(text, "!")
        && trim(alias) == alias && trim("  hi  ") == "hi"
        && unchanged == alias && whole_parts == [alias] && shortened == "small"
        && uppercase("ß中") == "SS中" && lowercase("İΣ") == "i̇ς"
        && split("", "") == ["", ""] && lines("") == []
        && lines("a\r\nb\n") == ["a", "b"];
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
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    let artifact = checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    let encoded =
        checkpoint_file_to_bytes(artifact.clone(), CheckpointFileLimits::default()).unwrap();
    let decoded = checkpoint_file_from_bytes(&encoded, CheckpointFileLimits::default()).unwrap();
    let snapshot = checkpoint_from_json(&decoded, &checked).unwrap();
    for saved in [&result.checkpoints[0], &snapshot] {
        let resumed = Interpreter
            .resume_checkpoint(&checked, saved, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(
            checkpoint_artifact_json(&[], "main", saved).unwrap(),
            artifact
        );
    }
}

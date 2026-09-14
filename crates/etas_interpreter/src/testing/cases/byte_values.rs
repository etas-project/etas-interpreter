use super::super::*;
use crate::api::codec::{
    CheckpointFileLimits, checkpoint_artifact_json, checkpoint_file_from_bytes,
    checkpoint_file_to_bytes, checkpoint_from_json,
};

#[tokio::test(flavor = "current_thread")]
async fn byte_assignment_preserves_aliases_through_handler_error_and_cancelled_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.codec.text.utf8_encode;
import std.runtime.checkpoint;
effect Select { action index() -> usize; }
flow main() -> bool {
    let old = utf8_encode("old");
    let new = utf8_encode("new");
    var values = [old];
    let alias = values;
    handle {
        values[0] = values[9];
    } with {
        Error<IndexError>.raise(_) => { finish (); }
    };
    if values != [old] { return false; }
    handle {
        values[perform Select.index()] = new;
    } with {
        Select.index() => { checkpoint("byte-index"); resume 0; }
    };
    return values == [new] && alias == [old];
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
    assert_eq!(result.checkpoints.len(), 1);
    let artifact = checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    let snapshot = checkpoint_from_json(&artifact, &checked).unwrap();
    let invocation = Interpreter.create_resume(&checked, &snapshot, &host, RunOptions::default());
    invocation
        .control()
        .stop(etas_host::execution::CancellationReason::Requested)
        .unwrap();
    let cancelled = invocation.execute().await.unwrap();
    assert!(matches!(
        cancelled.outcome,
        crate::api::RunOutcome::Cancelled(_)
    ));
    assert_eq!(
        checkpoint_artifact_json(&[], "main", &snapshot).unwrap(),
        artifact
    );
    let result = Interpreter
        .resume_checkpoint(&checked, &snapshot, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn shared_bytes_preserve_aliases_and_nested_values_across_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.bytes.len;
import std.codec.text.utf8_encode;
import std.runtime.checkpoint;
type Row = { body: bytes }
flow main() -> bool {
    var data = utf8_encode("abcd");
    let alias = data;
    let row = Row { body = data };
    let wrapped = Some(Ok<bytes, string>(data));
    var rows = [row];
    checkpoint("byte-aliases");
    rows[0].body = utf8_encode("replacement");
    data = utf8_encode("new");
    return len(alias) == 4 && alias[0] == 97
        && row.body == alias && wrapped == Some(Ok<bytes, string>(alias))
        && len(rows[0].body) == 11 && len(data) == 3;
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

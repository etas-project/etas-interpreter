use super::super::*;
use crate::api::codec::{
    CheckpointFileLimits, checkpoint_artifact_json, checkpoint_file_from_bytes,
    checkpoint_file_to_bytes, checkpoint_from_json,
};

#[tokio::test(flavor = "current_thread")]
async fn large_payload_checkpoint_preserves_nested_aliases_and_early_loop_cursor() {
    let checked = checked_project(
        r#"
module app.main;
import std.bytes.len as byte_len;
import std.text.len as text_len;
import std.codec.text.utf8_encode;
import std.runtime.checkpoint;
type Row = { body: bytes, text: string }
flow main(input: bytes, text: string, length: usize) -> bool {
    var rows = [Row { body = input, text = text }];
    let original = rows;
    let retained = Some(rows[0]);
    for row in rows limit Iterations(2) {
        checkpoint("large-capture");
        rows[0].body = utf8_encode("new");
        rows[0].text = "new";
        if row.body != input || row.text != text { return false; }
        break;
    }
    let retained_ok = match retained {
        Some(row) => row.body == input && row.text == text,
        None => false,
    };
    return original[0].body == input && original[0].text == text
        && byte_len(input) == length && text_len(text) == length
        && rows[0].body == utf8_encode("new") && rows[0].text == "new"
        && retained_ok;
}
"#,
    );
    for (length, fits_default_file_budget) in [(16 * 1024, true), (64 * 1024, false)] {
        let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![
                    InterpValue::Bytes(vec![7; length].into()),
                    InterpValue::String("x".repeat(length).into()),
                    InterpValue::usize(length),
                ],
                &host,
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(result.checkpoints.len(), 1);
        let saved = &result.checkpoints[0];
        let before_args = saved.args.clone();
        let before_budget = saved.host_state.budget.clone();
        assert!(
            matches!(&before_args[0], crate::orchestration::ValueSnapshot::Bytes(value) if value.len() == length)
        );
        assert!(
            matches!(&before_args[1], crate::orchestration::ValueSnapshot::String(value) if value.len() == length)
        );
        for _ in 0..2 {
            let resumed = Interpreter
                .resume_checkpoint(&checked, saved, &host, RunOptions::default())
                .await
                .unwrap();
            assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
            assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
            assert_eq!(saved.args, before_args);
            assert_eq!(saved.host_state.budget, before_budget);
        }
        let encoded = checkpoint_artifact_json(&[], "main", saved);
        if !fits_default_file_budget {
            let error = encoded.unwrap_err();
            assert!(
                error
                    .message()
                    .contains("snapshot expansion exceeds node budget"),
                "{error}"
            );
            assert_eq!(saved.args, before_args);
            assert_eq!(saved.host_state.budget, before_budget);
            continue;
        }
        let before = encoded.unwrap();
        let bytes =
            checkpoint_file_to_bytes(before.clone(), CheckpointFileLimits::default()).unwrap();
        let decoded = checkpoint_file_from_bytes(&bytes, CheckpointFileLimits::default()).unwrap();
        let restored = checkpoint_from_json(&decoded, &checked).unwrap();
        let resumed = Interpreter
            .resume_checkpoint(&checked, &restored, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(
            checkpoint_artifact_json(&[], "main", &restored).unwrap(),
            before
        );
        assert_eq!(
            checkpoint_artifact_json(&[], "main", saved).unwrap(),
            before
        );
    }
}

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

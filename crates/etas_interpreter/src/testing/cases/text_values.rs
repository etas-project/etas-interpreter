use super::super::*;
use crate::api::codec::{
    CheckpointFileLimits, checkpoint_artifact_json, checkpoint_file_from_bytes,
    checkpoint_file_to_bytes, checkpoint_from_json,
};

#[tokio::test(flavor = "current_thread")]
async fn stopping_a_host_suspended_assignment_preserves_its_old_binding_and_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
effect Append { action suffix() -> string; }
flow main() -> bool {
    var text = "old";
    let alias = text;
    handle { text = text + perform Append.suffix(); } with {
        Append.suffix() => {
            checkpoint("assignment-before-host");
            println("RHS waiting");
            if text != alias { abort("assignment was committed before RHS completed"); }
            resume "new";
        }
    };
    return text == "oldnew" && alias == "old";
}
"#,
    );
    let services = availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]);
    let mut host = FakeHost::new(services);
    let gate = host.pause_console_completion();
    let invocation = Interpreter.create_run(
        &checked,
        EntryPoint {
            item: checked.entry.unwrap(),
        },
        vec![],
        &host,
        RunOptions::default(),
    );
    let control = invocation.control();
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(invocation.execute(), async {
            gate.started.notified().await;
            control
                .stop(etas_host::execution::CancellationReason::Requested)
                .unwrap();
            gate.release.add_permits(1);
        })
    })
    .await
    .expect("Host completion must remain pollable during assignment cancellation");
    let result = result.unwrap();
    assert!(matches!(
        result.outcome,
        crate::api::RunOutcome::Cancelled(_)
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(result.value().is_none());
    assert_eq!(host.console_call_count(), 1);
    assert_eq!(result.checkpoints.len(), 1);
    let saved = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&[], "main", saved).unwrap();
    let bytes =
        checkpoint_file_to_bytes(artifact.clone(), CheckpointFileLimits::default()).unwrap();
    let decoded = checkpoint_file_from_bytes(&bytes, CheckpointFileLimits::default()).unwrap();
    let restored = checkpoint_from_json(&decoded, &checked).unwrap();
    for checkpoint in [saved, &restored] {
        let host = FakeHost::new(services);
        let resumed = Interpreter
            .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(host.stdout_text(), "RHS waiting\n");
        assert_eq!(host.console_call_count(), 1);
        assert_eq!(
            checkpoint_artifact_json(&[], "main", checkpoint).unwrap(),
            artifact
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn text_reassignment_keeps_old_values_across_host_error_handler_and_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
effect Append { action suffix() -> string; }
type Row = { text: string }
flow main(input: Array<i32>, expected: string) -> bool {
    var text = "";
    for _ in input limit Iterations(128) { text = text + "中😀"; }
    let old = text;
    var rows = [Row { text = text }];
    let before = rows;
    handle {
        text = text + perform Append.suffix();
    } with {
        Append.suffix() => {
            println("suffix");
            checkpoint("before-commit");
            if text != expected { abort("old binding lost during RHS evaluation"); }
            text = "handler changed the slot after the left operand was read";
            resume "!";
        }
    };
    if text != expected + "!" { return false; }
    let missing: Array<string> = [];
    handle { text = text + missing[1]; } with {
        Error<IndexError>.raise(_) => { finish (); }
    };
    rows[0].text = rows[0].text + "nested";
    return text == expected + "!" && old == expected
        && before[0].text == expected && rows[0].text == expected + "nested";
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Console,
        HostRequirementKind::Checkpoint,
    ]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![
                InterpValue::Array(
                    (0..64)
                        .map(|_| InterpValue::i32(0))
                        .collect::<Vec<_>>()
                        .into(),
                ),
                InterpValue::String("中😀".repeat(64).into()),
            ],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(host.stdout_text(), "suffix\n");
    assert_eq!(result.checkpoints.len(), 1);
    let saved = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&[], "main", saved).unwrap();
    let bytes =
        checkpoint_file_to_bytes(artifact.clone(), CheckpointFileLimits::default()).unwrap();
    let decoded = checkpoint_file_from_bytes(&bytes, CheckpointFileLimits::default()).unwrap();
    let restored = checkpoint_from_json(&decoded, &checked).unwrap();
    let invocation = Interpreter.create_resume(&checked, &restored, &host, RunOptions::default());
    invocation
        .control()
        .stop(etas_host::execution::CancellationReason::Requested)
        .unwrap();
    assert!(matches!(
        invocation.execute().await.unwrap().outcome,
        crate::api::RunOutcome::Cancelled(_)
    ));
    for checkpoint in [saved, &restored] {
        let resumed_host = FakeHost::new(availability(&[
            HostRequirementKind::Console,
            HostRequirementKind::Checkpoint,
        ]));
        let resumed = Interpreter
            .resume_checkpoint(&checked, checkpoint, &resumed_host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(
            resumed_host.stdout_text(),
            "",
            "do not repeat the completed host boundary"
        );
        assert_eq!(
            checkpoint_artifact_json(&[], "main", checkpoint).unwrap(),
            artifact
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn borrowed_join_preserves_suspended_argument_order_and_input_aliases() {
    let checked = checked_project(
        r#"
module app.main;
import std.text.join;
import std.io.println;
import std.runtime.checkpoint;
flow parts() -> Array<string> {
    println("parts");
    checkpoint("parts");
    return ["中", "😀", ""];
}
flow separator() -> string {
    println("separator");
    checkpoint("separator");
    return "|";
}
flow main() -> bool {
    let values = parts();
    let alias = values;
    let rendered = join(values, separator());
    return rendered == "中|😀|" && join(alias, "") == "中😀";
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
    assert_eq!(host.stdout_text(), "parts\nseparator\n");
    assert_eq!(result.checkpoints.len(), 2);
    for (index, checkpoint) in result.checkpoints.iter().enumerate() {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
        let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
        let host = FakeHost::new(services);
        let resumed = Interpreter
            .resume_checkpoint(&checked, &decoded, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), result.value());
        assert_eq!(
            host.stdout_text(),
            if index == 0 { "separator\n" } else { "" }
        );
    }
}

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

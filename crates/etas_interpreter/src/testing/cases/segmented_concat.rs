use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn segmented_rhs_preserves_values_during_handler_checkpoint_error_and_cancelled_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
effect Append { action suffix() -> string; }
type Row = { text: string }
flow build(prefix: string) -> string {
    var text = prefix;
    var i = 0;
    while i < 64 limit Iterations(128) {
        text = text + "中😀";
        i = i + 1;
    }
    return text;
}
flow main() -> bool {
    var text = build("L");
    let right = build("R");
    let left_alias = text;
    let right_alias = right;
    var rows = [Row { text = text }];
    let rows_alias = rows;
    handle { text = text + perform Append.suffix(); } with {
        Append.suffix() => {
            println("segmented rhs");
            checkpoint("pending-segmented-concat");
            if text != left_alias || right != right_alias { abort("operand changed before commit"); }
            text = "handler changed the slot after its operand was read";
            resume right;
        }
    };
    let combined = text;
    let missing: Array<string> = [];
    handle { text = text + missing[1]; } with {
        Error<IndexError>.raise(_) => { finish (); }
    };
    rows[0].text = rows[0].text + right;
    return text == left_alias + right_alias && combined == text
        && rows[0].text == text && rows_alias[0].text == left_alias
        && left_alias == build("L") && right_alias == build("R");
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
    assert_eq!(host.stdout_text(), "segmented rhs\n");
    assert_eq!(result.checkpoints.len(), 1);
    let saved = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&[], "main", saved).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    let invocation = Interpreter.create_resume(&checked, &decoded, &host, RunOptions::default());
    invocation
        .control()
        .stop(etas_host::execution::CancellationReason::Requested)
        .unwrap();
    assert!(matches!(
        invocation.execute().await.unwrap().outcome,
        crate::api::RunOutcome::Cancelled(_)
    ));
    for checkpoint in [saved, &decoded] {
        let host = FakeHost::new(services);
        let resumed = Interpreter
            .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), result.value());
        assert_eq!(host.stdout_text(), "");
        assert_eq!(
            checkpoint_artifact_json(&[], "main", checkpoint).unwrap(),
            artifact
        );
    }
}

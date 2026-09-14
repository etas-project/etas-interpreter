use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn assignment_commit_follows_resumed_handler_index() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
effect Select { action index() -> usize; }
flow main() -> bool {
    var values = ["old"];
    let alias = values;
    handle {
        values[perform Select.index()] = "new";
    } with {
        Select.index() => { checkpoint("index"); resume 0; }
    };
    return values == ["new"] && alias == ["old"];
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
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn assignment_retains_owned_rhs_and_aliases_across_index_checkpoints() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
flow rhs() -> string {
    println("rhs");
    return "new";
}
flow index(label: string, value: usize) -> usize {
    println(label);
    checkpoint(label);
    return value;
}
flow main() -> bool {
    var values = [["old", "keep"]];
    let alias = values;
    values[index("outer", 0)][index("inner", 1)] = rhs();
    return values == [["old", "new"]] && alias == [["old", "keep"]];
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
    assert_eq!(host.stdout_text(), "rhs\nouter\ninner\n");
    assert_eq!(result.checkpoints.len(), 2);
    for (snapshot, expected_output) in result.checkpoints.iter().zip(["inner\n", ""]) {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", snapshot).unwrap();
        let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
        let host = FakeHost::new(services);
        let invocation =
            Interpreter.create_resume(&checked, &checkpoint, &host, RunOptions::default());
        invocation
            .control()
            .stop(etas_host::execution::CancellationReason::Requested)
            .unwrap();
        let cancelled = invocation.execute().await.unwrap();
        assert!(matches!(
            cancelled.outcome,
            crate::api::RunOutcome::Cancelled(_)
        ));
        assert_eq!(host.stdout_text(), "");
        // Cancellation of one invocation cannot consume the checkpoint's pending assignment.
        assert_eq!(
            checkpoint_artifact_json(&["main.es".into()], "main", &checkpoint).unwrap(),
            artifact
        );
        let resumed = Interpreter
            .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
        assert_eq!(host.stdout_text(), expected_output);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn indexed_map_updates_preserve_aliases_order_and_checkpoint_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
flow main() -> bool {
    var values = { "a" => [1], "b" => [2] };
    let alias = values;
    if values.get("a") != Some([1]) { return false; }
    values["a"][0] = 3;
    values["c"] = [4];
    checkpoint("indexed-map");
    var order = "";
    var original_b = false;
    for (key, value) in values limit Iterations(8) {
        order = order + key;
        values[key] = [9];
        if key == "a" { values["b"] = [99]; checkpoint("map-cursor"); }
        if key == "b" { original_b = value == [2]; }
    }
    return original_b && order == "abc" && alias.get("a") == Some([1])
        && alias.get("b") == Some([2]) && alias.get("c") == None
        && values.get("a") == Some([9]) && values.get("b") == Some([9])
        && values.get("c") == Some([9]) && values.get("missing") == None;
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
    assert_eq!(result.checkpoints.len(), 2);
    for saved in &result.checkpoints {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap();
        let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
        for checkpoint in [saved, &decoded] {
            let resumed = Interpreter
                .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
                .await
                .unwrap();
            assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
            assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
        }
    }
}

use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn source_flow_reexports_with_inferred_effects_execute_and_resume_canonical_bodies() {
    let checked = checked_project_sources(vec![
        (
            SourceId(0),
            "src/app/main.es",
            r#"
module app.main;
import lib.api.exposed as invoke;
flow main() -> i32 {
    let callback = invoke;
    let first = invoke();
    return first + callback();
}
"#,
        ),
        (
            SourceId(1),
            "src/lib/leaf.es",
            r#"
module lib.leaf;
import std.io.println;
import std.runtime.checkpoint;
public flow emit() -> i32 {
    println("leaf");
    checkpoint("leaf");
    return 7;
}
"#,
        ),
        (
            SourceId(2),
            "src/lib/facade.es",
            "module lib.facade; public import lib.leaf.emit as exposed;",
        ),
        (
            SourceId(3),
            "src/lib/api.es",
            "module lib.api; public import lib.facade.*;",
        ),
        (
            SourceId(4),
            "src/lib/unrelated.es",
            "module lib.unrelated; public flow emit() -> i32 { return 99; }",
        ),
    ]);
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
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::i32(14)));
    assert_eq!(host.stdout_text(), "leaf\nleaf\n");
    assert_eq!(result.checkpoints.len(), 2);
    for (index, saved) in result.checkpoints.iter().enumerate() {
        let artifact =
            checkpoint_artifact_json(&["src/app/main.es".into()], "main", saved).unwrap();
        let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
        let host = FakeHost::new(availability(&[
            HostRequirementKind::Console,
            HostRequirementKind::Checkpoint,
        ]));
        let resumed = Interpreter
            .resume_checkpoint(&checked, &decoded, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&InterpValue::i32(14)));
        assert_eq!(host.stdout_text(), if index == 0 { "leaf\n" } else { "" });
        assert_eq!(
            checkpoint_artifact_json(&["src/app/main.es".into()], "main", saved).unwrap(),
            artifact
        );
    }
}

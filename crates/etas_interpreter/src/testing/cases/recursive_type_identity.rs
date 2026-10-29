use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn nonuniform_generic_recursion_constructs_closed_identities_and_resumes() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
enum Growing<T> { Leaf(T), Next(Growing<List<T>>) }
flow grow<T>(value: T, depth: i32) -> Growing<T> {
    if depth == 0 { return Growing.Leaf(value); }
    return Growing.Next(grow([value;], depth - 1));
}
flow main() -> Growing<i32> {
    let value = grow(7, 4);
    checkpoint("closed-recursive-identity");
    return value;
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
    assert!(result.value().is_some());
    assert_eq!(result.checkpoints.len(), 1);
    let saved = &result.checkpoints[0];
    let artifact = checkpoint_artifact_json(&[], "main", saved).unwrap();
    let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
    for checkpoint in [saved, &decoded] {
        let resumed = Interpreter
            .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), result.value());
    }
}

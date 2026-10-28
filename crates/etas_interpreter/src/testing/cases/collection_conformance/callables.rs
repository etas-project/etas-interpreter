use super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn collection_function_values_preserve_checked_abi_and_checkpoint_identity() {
    let checked = checked_project(
        r#"
module app.main;
import std.collections.{array_get, array_at, list_extend};
import std.runtime.checkpoint;
flow join<T>(left: List<T>, right: List<T>) -> List<T> { return list_extend(left, right); }
flow main() -> bool {
    let getter: (Array<i32>, i32) -> Option<i32> ![] = array_get;
    let indexer: (Array<i32>, i32) -> i32 ![Error<IndexError>] = array_at;
    let xs = [4, 5];
    checkpoint("collection-callables");
    let success = indexer(xs, 1)?;
    let failed = indexer(xs, 9)?;
    return getter(xs, 1) == Some(5) && getter(xs, -1) == None && success == Ok(5)
        && match failed { Ok(_) => false, Err(_) => true }
        && join([3; 4], [1; 2]) == [1; 2; 3; 4] && xs == [4, 5];
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
    let saved = &result.checkpoints[0];
    let wire = checkpoint_artifact_json(&["main.es".into()], "main", saved).unwrap();
    let decoded = checkpoint_from_json(&wire, &checked).unwrap();
    for snapshot in [saved, &decoded] {
        let resumed = Interpreter
            .resume_checkpoint(&checked, snapshot, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), result.value());
    }
}

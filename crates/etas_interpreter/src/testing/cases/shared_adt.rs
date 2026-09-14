use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn deep_recursive_adt_checkpoint_restores_in_memory() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
enum Link { End, Next(Link) }
flow main() -> i32 {
    var chain = Link.End;
    var count: i32 = 0;
    while count < 1000 limit Iterations(2000) {
        chain = Link.Next(chain);
        count = count + 1;
    }
    checkpoint("deep-adt");
    var visited: i32 = 0;
    while visited < count limit Iterations(2000) {
        match chain {
            Link.Next(next) => { chain = next; visited = visited + 1; },
            Link.End => { return -1; },
        }
    }
    return match chain { Link.End => visited, Link.Next(_) => -1 };
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let first = Interpreter
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
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.value(), Some(&InterpValue::i32(1000)));
    assert_eq!(first.checkpoints.len(), 1);
    let resumed = Interpreter
        .resume_checkpoint(
            &checked,
            &first.checkpoints[0],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), first.value());
}

#[tokio::test(flavor = "current_thread")]
async fn shared_recursive_adts_keep_aliases_during_suspended_nominal_assignment() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
enum Tree<T> { Leaf(T), Branch { left: Tree<T>, right: Tree<T> } }
type Row = { values: Array<Tree<i32>> }
effect Select { action index() -> usize; }
flow sum(tree: Tree<i32>) -> i32 {
    return match tree {
        Tree.Leaf(value) => value,
        Tree.Branch { left, right } => sum(left) + sum(right),
    };
}
flow main() -> bool {
    let leaf = Tree.Leaf(3);
    let branch = Tree.Branch { right = leaf, left = leaf };
    var row = Row { values = [branch] };
    let alias = row;
    let wrapped = Some(Ok<Tree<i32>, string>(branch));
    handle {
        row.values[perform Select.index()] = Tree.Leaf(9);
    } with {
        Select.index() => { checkpoint("pending-adt-write"); resume 0; }
    };
    return sum(row.values[0]) == 9 && sum(alias.values[0]) == 6
        && sum(branch) == 6 && wrapped == Some(Ok<Tree<i32>, string>(branch));
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let first = Interpreter
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
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.value(), Some(&InterpValue::Bool(true)));
    assert_eq!(first.checkpoints.len(), 1);
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &first.checkpoints[0]).unwrap();
    let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
    let invocation = Interpreter.create_resume(&checked, &checkpoint, &host, RunOptions::default());
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
        checkpoint_artifact_json(&["main.es".into()], "main", &checkpoint).unwrap(),
        artifact
    );
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&InterpValue::Bool(true)));
}

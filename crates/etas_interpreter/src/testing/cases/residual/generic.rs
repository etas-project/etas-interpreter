use super::*;

const GENERIC: &str = r#"
module app.main;
import std.runtime.checkpoint;
effect Gate { action check<T>(value: T) -> unit; action send<T>(value: T) -> unit; }
spec Safe<T>: trace = Gate.check<T> >> Gate.send<T>;
flow guarded<T>(value: T) -> i32 ![] ~ Safe<T> {
    var sent = 0;
    handle {
        perform Gate.check<T>(value);
        perform Gate.send<T>(value);
    } with {
        Gate.check<T>(_) => { resume (); }
        Gate.send<T>(_) => { sent = sent + 1; resume (); }
    };
    return sent;
}
flow main() -> i32 ![] {
    return guarded<i32>(7) + guarded<string>("seven");
}
"#;

#[tokio::test(flavor = "current_thread")]
async fn generic_trace_scopes_bind_each_checked_call_independently() {
    let checked = residual_checked(GENERIC);
    let host = FakeHost::new(availability(&[]));
    let result = run(&checked, &host).await;
    assert_eq!(
        result.value(),
        Some(&value::InterpValue::i32(2)),
        "{:#?}",
        result.diagnostics
    );
    assert!(result.diagnostics.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn generic_trace_checkpoint_restores_the_checked_application() {
    let checked = residual_checked(&GENERIC.replace(
        "perform Gate.send<T>(value);",
        "checkpoint(\"bound\"); perform Gate.send<T>(value);",
    ));
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let result = run(&checked, &host).await;
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.checkpoints.len(), 2);
    let saved =
        crate::api::codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&saved, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:#?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&value::InterpValue::i32(2)));
}

#[tokio::test(flavor = "current_thread")]
async fn compound_trace_selectors_preserve_nominal_and_recursive_type_arguments() {
    let checked = residual_checked(
        r#"
module app.main;
import std.runtime.checkpoint;
type Label = string;
enum Tree<T> { Leaf(T), Branch(Array<Tree<T>>), }
effect Gate { action check<T>(value: T) -> unit; action send<T>(value: T) -> unit; }
spec Safe<T>: trace = Gate.check<Array<T>> >> Gate.send<Array<T>>;
flow guarded<T>(value: T) -> i32 ![] ~ Safe<T> {
    var sent = 0;
    handle {
        perform Gate.check<Array<T>>([value]);
        perform Gate.send<Array<T>>([value]);
    } with {
        Gate.check<Array<T>>(_) => { checkpoint("compound"); resume (); }
        Gate.send<Array<T>>(_) => { sent = sent + 1; resume (); }
    };
    return sent;
}
flow main() -> i32 ![] {
    let leaf: Tree<i32> = Tree.Leaf(7);
    let branch: Tree<i32> = Tree.Branch([leaf]);
    return guarded<Label>(Label("seven")) + guarded<Tree<i32>>(branch);
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let result = run(&checked, &host).await;
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(2)));
    assert_eq!(result.checkpoints.len(), 2);
    let saved = crate::api::codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = crate::api::codec::checkpoint_from_json(&saved, &checked).unwrap();
    let resumed = Interpreter.resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default()).await.unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:#?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&value::InterpValue::i32(2)));
}

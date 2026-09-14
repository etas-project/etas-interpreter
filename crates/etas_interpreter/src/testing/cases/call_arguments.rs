use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn shared_call_descriptors_preserve_callee_argument_and_variant_order_on_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
enum Pair { Values(i32, i32), }
flow consume(a: i32, b: i32) -> i32 { return a * 10 + b; }
flow choose() -> (i32, i32) -> i32 {
    println("callee");
    checkpoint("callee");
    return (a: i32, b: i32) => consume(a, b);
}
flow argument(label: string, value: i32) -> i32 {
    println(label);
    checkpoint(label);
    return value;
}
flow main() -> i32 {
    let first = consume(a = argument("a", 1), b = argument("b", 2));
    let second = choose()(argument("c", 3), argument("d", 4));
    let pair = Pair.Values(argument("e", 5), argument("f", 6));
    return match pair { Pair.Values(a, b) => first * 10000 + second * 100 + a * 10 + b };
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
    assert_eq!(result.value(), Some(&InterpValue::i32(123456)));
    let labels = ["a", "b", "callee", "c", "d", "e", "f"];
    assert_eq!(host.stdout_text(), labels.join("\n") + "\n");
    assert_eq!(result.checkpoints.len(), labels.len());
    for (index, checkpoint) in result.checkpoints.iter().enumerate() {
        let artifact = checkpoint_artifact_json(&["main.es".into()], "main", checkpoint).unwrap();
        let decoded = checkpoint_from_json(&artifact, &checked).unwrap();
        for snapshot in [checkpoint, &decoded] {
            let host = FakeHost::new(services);
            let resumed = Interpreter
                .resume_checkpoint(&checked, snapshot, &host, RunOptions::default())
                .await
                .unwrap();
            assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
            assert_eq!(resumed.value(), result.value());
            let expected: String = labels[index + 1..]
                .iter()
                .map(|s| format!("{s}\n"))
                .collect();
            assert_eq!(host.stdout_text(), expected, "checkpoint {index}");
        }
    }
}

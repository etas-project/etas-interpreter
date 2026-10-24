use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn prompt_alias_and_pending_prefix_survive_append_and_file_resume() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
flow text() -> string {
    println("before");
    checkpoint("prefix");
    println("after");
    return "added";
}
flow main() -> (Prompt, Prompt) {
    let original = Prompt.new().user(Public("prefix"));
    let changed = original.assistant(text());
    return (original, changed);
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
    assert_eq!(host.stdout_text(), "before\nafter\n");
    assert_eq!(result.checkpoints.len(), 1);
    let prefix = crate::value::PromptMessage {
        role: crate::value::PromptRole::User,
        text: "prefix".into(),
        trust: Some(etas_types::TrustWrapper::Public),
    };
    let added = crate::value::PromptMessage {
        role: crate::value::PromptRole::Assistant,
        text: "added".into(),
        trust: None,
    };
    let expected = InterpValue::Tuple(
        vec![
            InterpValue::Prompt(vec![prefix.clone()].into()),
            InterpValue::Prompt(vec![prefix, added].into()),
        ]
        .into(),
    );
    assert_eq!(result.value(), Some(&expected));
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    let bytes = crate::api::codec::checkpoint_file_to_bytes(artifact, Default::default()).unwrap();
    let document =
        crate::api::codec::checkpoint_file_from_bytes(&bytes, Default::default()).unwrap();
    let decoded = checkpoint_from_json(&document, &checked).unwrap();
    for checkpoint in [&result.checkpoints[0], &decoded] {
        let host = FakeHost::new(services);
        let resumed = Interpreter
            .resume_checkpoint(&checked, checkpoint, &host, RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), Some(&expected));
        assert_eq!(host.stdout_text(), "after\n");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_data_nominal_payload_resumes_once_and_keeps_canonical_json() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
import std.runtime.checkpoint;
type Row = { title: string, values: List<i32> };
flow data() -> Row {
    println("before");
    checkpoint("data");
    println("after");
    return Row { title = "λ", values = [1; 2] };
}
flow main() -> Prompt { return Prompt.new().data(data()); }
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
    let expected = InterpValue::Prompt(
        vec![crate::value::PromptMessage {
            role: crate::value::PromptRole::Data,
            text: "{\"title\":\"λ\",\"values\":[1,2]}".into(),
            trust: None,
        }]
        .into(),
    );
    assert_eq!(result.value(), Some(&expected));
    assert_eq!(host.stdout_text(), "before\nafter\n");
    assert_eq!(result.checkpoints.len(), 1);
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &result.checkpoints[0]).unwrap();
    let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
    let host = FakeHost::new(services);
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&expected));
    assert_eq!(host.stdout_text(), "after\n");
}

use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn model_payload_survives_checked_call_checkpoint_and_file_resume_without_reexecution() {
    let checked = checked_project(
        r#"
module app.main;
import std.agent.prompt.Prompt;
import std.runtime.checkpoint;
agent Writer(input: string) -> ModelResponse {
    return Prompt.new().user(Public(input));
}
flow main() -> ModelResponse {
    let response = Writer.run("payload");
    checkpoint("model-payload");
    return response;
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Agentic,
        HostRequirementKind::Checkpoint,
    ]));
    let expected = value::HostSupportValue::Record(
        (0..128)
            .map(|i| {
                (
                    format!("field-{i}"),
                    value::HostSupportValue::Variant {
                        name: "Payload".into(),
                        fields: vec![
                            value::HostSupportValue::String("x".repeat(1024).into()),
                            value::HostSupportValue::Bytes(vec![7; 128].into()),
                        ]
                        .into(),
                    },
                )
            })
            .collect(),
    );
    let response = HostValue::Record(
        (0..128)
            .map(|i| {
                (
                    format!("field-{i}"),
                    HostValue::Variant {
                        name: "Payload".into(),
                        fields: vec![
                            HostValue::String("x".repeat(1024)),
                            HostValue::Bytes(vec![7; 128]),
                        ],
                    },
                )
            })
            .collect(),
    );
    host.seed_model_response_value(response);
    let mut options = RunOptions::default();
    options.host_context.authority.grants = vec![HostActionGrant::allow("Agentic", "infer")];
    options.model_policy.response_decode = api::ModelResponseDecodePolicy::ModelResponse;
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options.clone(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let Some(value::InterpValue::ModelResponse(response)) = first.value() else {
        panic!("model response");
    };
    assert_eq!(
        response.message.content,
        vec![value::ModelContentValue::Value(expected)]
    );
    assert_eq!(host.model_call_count(), 1);
    assert_eq!(first.checkpoints.len(), 1);
    let resumed = Interpreter
        .resume_checkpoint(&checked, &first.checkpoints[0], &host, options.clone())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), first.value());
    let artifact =
        api::codec::checkpoint_artifact_json(&[], "main", &first.checkpoints[0]).unwrap();
    let bytes =
        api::codec::checkpoint_file_to_bytes(artifact, api::codec::CheckpointFileLimits::default())
            .unwrap();
    let document =
        api::codec::checkpoint_file_from_bytes(&bytes, api::codec::CheckpointFileLimits::default())
            .unwrap();
    let checkpoint = api::codec::checkpoint_from_json(&document, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, options)
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), first.value());
    assert_eq!(host.model_call_count(), 1);
}

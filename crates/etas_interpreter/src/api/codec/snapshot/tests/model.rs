use super::*;
use crate::api::codec::CheckpointDocument;

#[test]
fn model_response_snapshot_preserves_nested_host_payloads_without_reserializing() {
    for depth in [1000, 4000] {
        let mut payload = HostJsonSupportValue::String("payload".into());
        for _ in 0..depth {
            payload = HostJsonSupportValue::Object(vec![("child".into(), payload)].into());
        }
        let value = InterpValue::ModelResponse(ModelResponseValue {
            id: 19,
            message: ModelMessageValue {
                role: ModelRoleValue::Assistant,
                content: vec![ModelContentValue::Value(HostSupportValue::Record(vec![(
                    "body".into(),
                    HostSupportValue::Json(payload.clone()),
                )]))],
            },
            tool_calls: vec![ModelToolCallValue {
                id: "call-1".into(),
                tool: "inspect".into(),
                args: HostSupportValue::List(vec![HostSupportValue::Json(payload)]),
            }],
            usage: Some(ModelUsageValue {
                input_tokens: 3,
                output_tokens: 5,
            }),
        });
        let saved = ValueSnapshot::capture(&value).unwrap();
        let (wire, cost) = measure(|| CheckpointDocument::from_value(encode(Node::Value(&saved))));
        assert!(
            cost.bytes <= depth * 4096 + 100_000,
            "snapshot model copies: {cost:?}"
        );
        // Deep serde_json::Value equality recurses. Compare decoded typed values,
        // whose JSON leaf comparison is iterative, instead.
        assert_eq!(codec::value_from_json(&wire).unwrap(), value);
        let mut artifact =
            json!({"schema":crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA,"checkpoint":null});
        artifact["checkpoint"] = wire.into_value();
        let bytes =
            codec::checkpoint_file_to_bytes(artifact, codec::CheckpointFileLimits::default())
                .unwrap();
        let decoded =
            codec::checkpoint_file_from_bytes(&bytes, codec::CheckpointFileLimits::default())
                .unwrap();
        assert_eq!(
            codec::value_from_json(&decoded["checkpoint"]).unwrap(),
            value
        );
        eprintln!("snapshot model depth={depth}: {cost:?}");
    }
}

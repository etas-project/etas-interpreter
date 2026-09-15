use super::*;
use crate::api::codec::CheckpointDocument;

fn model(payload: HostSupportValue) -> ValueSnapshot {
    ValueSnapshot::ModelResponse(ModelResponseValue {
        id: 1,
        message: ModelMessageValue {
            role: ModelRoleValue::Assistant,
            content: vec![ModelContentValue::Value(payload)],
        },
        tool_calls: vec![],
        usage: None,
    })
}

fn reject(
    value: &ValueSnapshot,
    limits: codec::CheckpointFileLimits,
    reason: &str,
) -> crate::testing::allocation::Allocations {
    measure(|| {
        let result =
            encode_with_budget(Node::Value(value), &mut EncodingBudget::with_limits(limits))
                .map(CheckpointDocument::from_value);
        let Err(error) = result else {
            panic!("model payload bypassed {reason} budget")
        };
        assert!(error.message().contains(reason), "{}", error.message());
    })
    .1
}

#[test]
fn model_snapshot_budget_rejects_host_fanout_and_bytes_before_allocation() {
    let limits = codec::CheckpointFileLimits {
        max_nodes: 32,
        ..Default::default()
    };
    let small = model(HostSupportValue::List(
        vec![HostSupportValue::Unit; 128].into(),
    ));
    let large = model(HostSupportValue::List(
        vec![HostSupportValue::Unit; 100_000].into(),
    ));
    let small = reject(&small, limits, "node");
    let large = reject(&large, limits, "node");
    assert_eq!(small.bytes, large.bytes, "allocated rejected fanout");
    assert!(
        large.bytes < 32 * 4096,
        "large allocation before admission: {large:?}"
    );
    assert_eq!(large.bytes, large.released_bytes);
    let bytes = reject(
        &model(HostSupportValue::Bytes(vec![0; 100_000].into())),
        limits,
        "node",
    );
    assert!(
        bytes.bytes < 32 * 4096,
        "allocated byte array before admission: {bytes:?}"
    );
    assert_eq!(bytes.bytes, bytes.released_bytes);
}

#[test]
fn model_snapshot_budget_charges_host_payloads_and_labels_before_copying() {
    let limits = codec::CheckpointFileLimits {
        max_bytes: 256,
        ..Default::default()
    };
    let values = [
        HostSupportValue::String("x".repeat(100_000).into()),
        HostSupportValue::Int("1".repeat(100_000).into()),
        HostSupportValue::UInt("1".repeat(100_000).into()),
        HostSupportValue::Bytes(vec![0; 100_000].into()),
        HostSupportValue::Record(vec![("x".repeat(100_000), HostSupportValue::Unit)].into()),
        HostSupportValue::Variant {
            name: "x".repeat(100_000).into(),
            fields: vec![].into(),
        },
    ];
    for payload in values {
        let cost = reject(&model(payload), limits, "byte");
        assert!(cost.bytes < 32 * 4096, "copied rejected payload: {cost:?}");
        assert_eq!(cost.bytes, cost.released_bytes);
    }
}

#[test]
fn model_snapshot_budget_is_shared_with_outer_values_and_repeated_json_occurrences() {
    let payload = HostJsonSupportValue::String("x".repeat(2048).into());
    let model = model(HostSupportValue::Json(payload.clone()));
    let limits = codec::CheckpointFileLimits {
        max_bytes: 3000,
        ..Default::default()
    };
    let wire = CheckpointDocument::from_value(
        encode_with_budget(
            Node::Value(&model),
            &mut EncodingBudget::with_limits(limits),
        )
        .unwrap(),
    );
    assert_eq!(wire["kind"], "model_response");
    let root = ValueSnapshot::Tuple(vec![model, ValueSnapshot::Json(payload)].into());
    let cost = reject(&root, limits, "byte");
    assert_eq!(cost.bytes, cost.released_bytes, "partial graph leaked");
}

#[test]
fn model_snapshot_budget_includes_content_text_and_tool_metadata() {
    let limits = codec::CheckpointFileLimits {
        max_bytes: 256,
        ..Default::default()
    };
    for field in ["text", "id", "tool"] {
        let ValueSnapshot::ModelResponse(mut value) = model(HostSupportValue::Unit) else {
            unreachable!()
        };
        if field == "text" {
            value.message.content = vec![ModelContentValue::Text("x".repeat(100_000))];
        } else {
            value.tool_calls = vec![ModelToolCallValue {
                id: if field == "id" {
                    "x".repeat(100_000)
                } else {
                    String::new()
                },
                tool: if field == "tool" {
                    "x".repeat(100_000)
                } else {
                    String::new()
                },
                args: HostSupportValue::Unit,
            }];
        }
        let cost = reject(&ValueSnapshot::ModelResponse(value), limits, "byte");
        assert!(cost.bytes < 32 * 4096, "copied rejected {field}: {cost:?}");
        assert_eq!(cost.bytes, cost.released_bytes);
    }
}

#[test]
fn model_snapshot_budget_reserves_content_and_tool_call_output_slots() {
    let limits = codec::CheckpointFileLimits {
        max_nodes: 32,
        ..Default::default()
    };
    for tool_calls in [false, true] {
        let costs = [128, 100_000].map(|count| {
            let ValueSnapshot::ModelResponse(mut value) = model(HostSupportValue::Unit) else {
                unreachable!()
            };
            if tool_calls {
                value.tool_calls = (0..count)
                    .map(|_| ModelToolCallValue {
                        id: String::new(),
                        tool: String::new(),
                        args: HostSupportValue::Unit,
                    })
                    .collect();
            } else {
                value.message.content = vec![ModelContentValue::Text(String::new()); count];
            }
            reject(&ValueSnapshot::ModelResponse(value), limits, "node")
        });
        assert_eq!(
            costs[0].bytes, costs[1].bytes,
            "allocated rejected model slots"
        );
        assert_eq!(costs[1].bytes, costs[1].released_bytes);
        assert!(costs[1].bytes < 4096);
    }
}

#[test]
fn model_snapshot_budget_bounds_shared_expansion_and_deep_partial_cleanup() {
    let mut shared = HostJsonSupportValue::Null;
    for _ in 0..16 {
        shared = HostJsonSupportValue::Array(vec![shared.clone(), shared].into());
    }
    let retained = shared.clone();
    let snapshot = model(HostSupportValue::Json(shared));
    let cost = reject(
        &snapshot,
        codec::CheckpointFileLimits {
            max_nodes: 256,
            ..Default::default()
        },
        "node",
    );
    assert!(
        cost.bytes < 256 * 4096,
        "expanded shared backing past budget: {cost:?}"
    );
    assert_eq!(cost.bytes, cost.released_bytes);
    let ValueSnapshot::ModelResponse(value) = &snapshot else {
        unreachable!()
    };
    let ModelContentValue::Value(HostSupportValue::Json(value)) = &value.message.content[0] else {
        unreachable!()
    };
    assert_eq!(value, &retained);
    eprintln!("model shared expansion budget=256: {cost:?}");

    let mut deep = HostJsonSupportValue::Null;
    for _ in 0..30_000 {
        deep = HostJsonSupportValue::Array(vec![deep].into());
    }
    let snapshot = model(HostSupportValue::Json(deep));
    let cost = reject(
        &snapshot,
        codec::CheckpointFileLimits {
            max_nodes: 20_000,
            ..Default::default()
        },
        "node",
    );
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "deep partial output leaked"
    );
    assert!(cost.bytes < 20_000 * 4096);
    eprintln!("model deep partial output budget=20000: {cost:?}");
}

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
                content: vec![ModelContentValue::Value(HostSupportValue::Record(
                    vec![("body".into(), HostSupportValue::Json(payload.clone()))].into(),
                ))],
            },
            tool_calls: vec![ModelToolCallValue {
                id: "call-1".into(),
                tool: "inspect".into(),
                args: HostSupportValue::List(vec![HostSupportValue::Json(payload)].into()),
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

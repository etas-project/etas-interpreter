use crate::api::codec::{self, CheckpointDocument};
use crate::testing::allocation::measure;
use crate::value::{HostJsonSupportValue as Json, HostSupportValue as Support, InterpValue};
use serde_json::{Value, json};

// Previous production algorithm, used only for bounded wire/cost comparisons.
fn legacy(value: &Support) -> Value {
    match value {
        Support::Unit => json!({"kind":"unit"}),
        Support::Bool(value) => json!({"kind":"bool","value":value}),
        Support::Int(value) => json!({"kind":"int","value":value}),
        Support::UInt(value) => json!({"kind":"uint","value":value}),
        Support::FloatBits(value) => json!({"kind":"float_bits","value":value}),
        Support::String(value) => json!({"kind":"string","value":value}),
        Support::Bytes(value) => json!({"kind":"bytes","value":value}),
        Support::List(values) => {
            json!({"kind":"list","values":values.iter().map(legacy).collect::<Vec<_>>()})
        }
        Support::Map(entries) => {
            json!({"kind":"map","entries":entries.iter().map(|(key,value)| json!({"key":legacy(key),"value":legacy(value)})).collect::<Vec<_>>()})
        }
        Support::Record(fields) => {
            json!({"kind":"record","fields":fields.iter().map(|(name,value)| json!({"name":name,"value":legacy(value)})).collect::<Vec<_>>()})
        }
        Support::Variant { name, fields } => {
            json!({"kind":"variant","name":name,"fields":fields.iter().map(legacy).collect::<Vec<_>>()})
        }
        Support::Json(value) => codec::json::tests::legacy_wrapped(value),
    }
}

fn chain(depth: usize, leaf: Support) -> Support {
    (0..depth).fold(leaf, |value, n| match n % 4 {
        0 => Support::List(vec![value]),
        1 => Support::Record(vec![("child".into(), value)]),
        2 => Support::Map(vec![(Support::String("key".into()), value)]),
        _ => Support::Variant {
            name: "Next".into(),
            fields: vec![value],
        },
    })
}

#[test]
fn host_container_encoding_removes_repeated_descendant_materialization() {
    for depth in [16, 32, 64] {
        let support = chain(depth, Support::String("p".repeat(1024)));
        let (expected, old) = measure(|| CheckpointDocument::from_value(legacy(&support)));
        let host = codec::host_value_from_json(&expected).unwrap();
        let (actual, cost) = measure(|| {
            CheckpointDocument::from_value(super::super::host_support_value_json(&support))
        });
        let (external, host_cost) =
            measure(|| CheckpointDocument::from_value(codec::host_value_json(&host)));
        assert_eq!(*actual, *expected);
        assert_eq!(*external, *expected);
        assert_eq!(
            super::super::host_support_value_from_json(&actual).unwrap(),
            support
        );
        for cost in [cost, host_cost] {
            assert!(
                cost.bytes < old.bytes / 2,
                "still copying descendant graphs: old={old:?}, new={cost:?}"
            );
            assert!(
                cost.bytes <= depth * 3072 + 8192,
                "nonlinear encoding: {cost:?}"
            );
        }
        eprintln!(
            "Host container depth={depth}: old={old:?}, support={cost:?}, host={host_cost:?}"
        );
    }
}

#[test]
fn host_container_encoding_preserves_all_wire_kinds_and_order() {
    let value = Support::Record(vec![
        (
            "z".into(),
            Support::List(vec![
                Support::Unit,
                Support::Bool(true),
                Support::Int(i128::MIN.to_string()),
                Support::UInt(u128::MAX.to_string()),
                Support::FloatBits(0x7ff8_0000_0000_0007),
                Support::FloatBits((-0.0f64).to_bits()),
                Support::String("\"\\\n\t".into()),
                Support::Bytes(vec![0, 127, 255]),
                Support::List(vec![]),
                Support::Map(vec![]),
                Support::Record(vec![]),
                Support::Variant {
                    name: "Nullary".into(),
                    fields: vec![],
                },
            ]),
        ),
        (
            "a".into(),
            Support::Map(vec![
                (
                    Support::Bytes(vec![1]),
                    Support::Json(Json::String("json".into())),
                ),
                (
                    Support::Bytes(vec![1]),
                    Support::Variant {
                        name: "V".into(),
                        fields: vec![Support::Bool(false)],
                    },
                ),
            ]),
        ),
        ("z".into(), Support::Unit),
    ]);
    let expected = legacy(&value);
    assert_eq!(super::super::host_support_value_json(&value), expected);
    let host = codec::host_value_from_json(&expected).unwrap();
    assert_eq!(codec::host_value_json(&host), expected);
    assert_eq!(
        super::super::host_support_value_from_json(&expected).unwrap(),
        value
    );
}

#[test]
fn wide_host_container_encoding_only_materializes_the_output_graph() {
    for count in [1000, 2000, 4000] {
        let support = Support::Record(
            (0..count)
                .map(|n| {
                    (
                        n.to_string(),
                        Support::Variant {
                            name: "Payload".into(),
                            fields: vec![
                                Support::String("x".repeat(1024)),
                                Support::Bytes(vec![7; 64]),
                            ],
                        },
                    )
                })
                .collect(),
        );
        let (expected, old) = measure(|| CheckpointDocument::from_value(legacy(&support)));
        let host = codec::host_value_from_json(&expected).unwrap();
        let (actual, cost) = measure(|| {
            CheckpointDocument::from_value(super::super::host_support_value_json(&support))
        });
        let (external, host_cost) =
            measure(|| CheckpointDocument::from_value(codec::host_value_json(&host)));
        assert_eq!(*actual, *expected);
        assert_eq!(*external, *expected);
        for cost in [cost, host_cost] {
            assert!(
                old.bytes > cost.bytes + count * 1024,
                "retained payload copy: old={old:?}, new={cost:?}"
            );
            assert!(
                cost.released_bytes < cost.bytes / 10,
                "intermediate graph: {cost:?}"
            );
        }
        eprintln!(
            "wide Host container n={count}: old={old:?}, support={cost:?}, host={host_cost:?}"
        );
    }
}

#[test]
fn host_encoding_does_not_coerce_invalid_abi_values_or_change_decode_errors() {
    let malformed = Support::Record(vec![("child".into(), Support::UInt("-1".into()))]);
    let wire = super::super::host_support_value_json(&malformed);
    assert_eq!(wire, legacy(&malformed));
    assert!(
        codec::host_value_from_json(&wire)
            .unwrap_err()
            .message()
            .contains("u128")
    );

    let mut wire = super::super::host_support_value_json(&chain(2, Support::Bytes(vec![1])));
    wire["fields"][0]["value"]["values"][0]["value"] = json!([256]);
    assert!(codec::host_value_from_json(&wire).is_err());
    assert!(super::super::host_support_value_from_json(&wire).is_err());
}

#[test]
fn model_container_encoding_keeps_deep_json_payload_stack_safe() {
    const WORKER: &str = "ETAS_TEST_MODEL_CONTAINER_CODEC_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let current = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "model container codec subprocess failed: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    use crate::value::{
        ModelContentValue, ModelMessageValue, ModelResponseValue, ModelRoleValue,
        ModelToolCallValue,
    };
    for depth in [1000, 4000, 30_000] {
        let mut payload = Json::String("payload".into());
        for _ in 0..depth {
            payload = Json::Array(vec![payload].into());
        }
        let support = chain(8, Support::Json(payload.clone()));
        let value = InterpValue::ModelResponse(ModelResponseValue {
            id: 1,
            message: ModelMessageValue {
                role: ModelRoleValue::Assistant,
                content: vec![ModelContentValue::Value(support.clone())],
            },
            tool_calls: vec![ModelToolCallValue {
                id: "call".into(),
                tool: "inspect".into(),
                args: support,
            }],
            usage: None,
        });
        let (wire, cost) = measure(|| CheckpointDocument::from_value(codec::value_json(&value)));
        assert!(
            cost.bytes <= depth * 4096 + 100_000,
            "model payload copies: {cost:?}"
        );
        assert_eq!(codec::value_from_json(&wire).unwrap(), value);
        eprintln!("model JSON payload depth={depth}: {cost:?}");
    }
}

use super::*;
use serde_json::json;

fn artifact(payload: Value) -> Value {
    let mut value =
        json!({"schema": crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA, "checkpoint": null});
    value["checkpoint"] = payload;
    value
}

#[test]
fn file_roundtrip_preserves_json_scalars_fields_and_order() {
    let value = artifact(json!({"text":"\"\\\n中文", "zero":-0.0, "max":u64::MAX,
        "values":[null,true,false,1,2.25], "empty":{}, "array":[], "object":{"a":2,"b":1}}));
    let bytes = checkpoint_file_to_bytes(value.clone(), CheckpointFileLimits::default()).unwrap();
    let decoded = checkpoint_file_from_bytes(&bytes, CheckpointFileLimits::default()).unwrap();
    assert_eq!(*decoded, value);
    assert_eq!(
        decoded["checkpoint"]["zero"].as_f64().unwrap().to_bits(),
        (-0.0_f64).to_bits()
    );
}

#[test]
fn deep_file_roundtrip_uses_bounded_json_nesting() {
    for depth in [1000, 4000, 30_000] {
        let mut value = json!("leaf");
        for _ in 0..depth {
            value = Value::Array(vec![value]);
        }
        let bytes =
            checkpoint_file_to_bytes(artifact(value), CheckpointFileLimits::default()).unwrap();
        assert!(bytes.len() < depth * 64 + 1024);
        let parsed: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["schema"], FILE_SCHEMA);
        let decoded = checkpoint_file_from_bytes(&bytes, CheckpointFileLimits::default()).unwrap();
        let mut value = &decoded["checkpoint"];
        for _ in 0..depth {
            let values = value.as_array().unwrap();
            assert_eq!(values.len(), 1);
            value = &values[0];
        }
        assert_eq!(value, "leaf");
    }
}

#[test]
fn file_versions_are_explicit_and_legacy_nested_artifacts_are_rejected() {
    let old = serde_json::to_vec(&artifact(json!({}))).unwrap();
    assert!(
        checkpoint_file_from_bytes(&old, CheckpointFileLimits::default())
            .err()
            .unwrap()
            .message()
            .contains("unsupported checkpoint file schema")
    );
    let unknown =
        serde_json::to_vec(&json!({"schema":"etas.interpreter.checkpoint-file.flat-json.v99"}))
            .unwrap();
    assert!(
        checkpoint_file_from_bytes(&unknown, CheckpointFileLimits::default())
            .err()
            .unwrap()
            .message()
            .contains("unsupported checkpoint file schema")
    );
    let old_machine = json!({"schema":"etas.cli.interpreter-checkpoint.v33"});
    assert!(
        checkpoint_file_to_bytes(old_machine, CheckpointFileLimits::default())
            .unwrap_err()
            .message()
            .contains("unsupported checkpoint artifact schema")
    );
}

#[test]
fn file_graph_rejects_cycles_aliases_orphans_duplicates_and_invalid_indices() {
    for (root, nodes) in [
        (0, json!([])),
        (1, json!([{"kind":"null"}])),
        (0, json!([{"kind":"array","value":[0]}])),
        (0, json!([{"kind":"array","value":[9]}])),
        (0, json!([{"kind":"array","value":[1,1]}, {"kind":"null"}])),
        (0, json!([{"kind":"array","value":[]}, {"kind":"null"}])),
        (
            0,
            json!([{"kind":"array","value":[1]}, {"kind":"array","value":[0]}]),
        ),
        (
            0,
            json!([{"kind":"object","value":[["a",1],["a",2]]}, {"kind":"null"}, {"kind":"null"}]),
        ),
    ] {
        let bytes =
            serde_json::to_vec(&json!({"schema":FILE_SCHEMA, "root":root,"nodes":nodes})).unwrap();
        let error = checkpoint_file_from_bytes(&bytes, CheckpointFileLimits::default())
            .err()
            .unwrap();
        assert!(
            !error.message().contains("missing `schema`"),
            "graph must be rejected before logical decoding: {}",
            error.message()
        );
    }
}

#[test]
fn file_resource_budgets_apply_to_both_directions() {
    let value = artifact(json!(["payload", 1, 2]));
    let bytes = checkpoint_file_to_bytes(value.clone(), CheckpointFileLimits::default()).unwrap();
    let file: File = serde_json::from_slice(&bytes).unwrap();
    let exact = CheckpointFileLimits {
        max_bytes: bytes.len(),
        max_nodes: file.nodes.len(),
    };
    assert_eq!(
        checkpoint_file_to_bytes(value.clone(), exact).unwrap(),
        bytes
    );
    assert_eq!(*checkpoint_file_from_bytes(&bytes, exact).unwrap(), value);
    for limits in [
        CheckpointFileLimits {
            max_bytes: bytes.len() - 1,
            ..exact
        },
        CheckpointFileLimits {
            max_nodes: file.nodes.len() - 1,
            ..exact
        },
        CheckpointFileLimits {
            max_bytes: 0,
            ..exact
        },
        CheckpointFileLimits {
            max_nodes: 0,
            ..exact
        },
    ] {
        assert!(checkpoint_file_to_bytes(value.clone(), limits).is_err());
        assert!(checkpoint_file_from_bytes(&bytes, limits).is_err());
    }
}

#[test]
fn encoder_rejection_releases_deep_input_without_recursive_drop() {
    let mut value = Value::Null;
    for _ in 0..30_000 {
        value = Value::Array(vec![value]);
    }
    let error = checkpoint_file_to_bytes(
        artifact(value),
        CheckpointFileLimits {
            max_nodes: 20,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.message().contains("node budget"));
}

#[test]
fn node_budget_rejects_before_allocating_the_entire_table() {
    let bytes = serde_json::to_vec(&json!({"schema":FILE_SCHEMA,"root":0,
        "nodes":(0..10_000).map(|_| json!({"kind":"null"})).collect::<Vec<_>>()
    }))
    .unwrap();
    let (result, allocations) = crate::testing::allocation::measure(|| {
        checkpoint_file_from_bytes(
            &bytes,
            CheckpointFileLimits {
                max_nodes: 1,
                ..Default::default()
            },
        )
    });
    assert!(result.err().unwrap().message().contains("node budget"));
    assert!(
        allocations.bytes < 16 * 1024,
        "decoder allocated the rejected table: {allocations:?}"
    );
}

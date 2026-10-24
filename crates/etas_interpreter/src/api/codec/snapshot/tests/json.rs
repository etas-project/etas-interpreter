use super::*;
use crate::api::codec::CheckpointDocument;
use crate::value::HostJsonSupportValue as Json;

fn chain(depth: usize) -> Json {
    let mut value = Json::String("p".repeat(1024).into());
    for _ in 0..depth {
        value = Json::Object(vec![("child".into(), value)].into());
    }
    value
}

#[test]
fn snapshot_json_encoding_does_not_reserialize_descendants() {
    for depth in [16, 32, 64] {
        let json = chain(depth);
        let saved = ValueSnapshot::Json(json.clone());
        let (actual, direct) =
            measure(|| CheckpointDocument::from_value(encode(Node::Value(&saved))));
        // Keep the former production algorithm as an explicit test-only baseline.
        let (expected, legacy) =
            measure(|| CheckpointDocument::from_value(codec::json::tests::legacy_wrapped(&json)));
        assert_eq!(*actual, *expected);
        assert!(
            legacy.bytes > direct.bytes + depth * 2048,
            "snapshot path still reserializes descendants at depth {depth}: direct={direct:?}, legacy={legacy:?}"
        );
        eprintln!("snapshot JSON depth={depth}: direct={direct:?}; legacy={legacy:?}");
    }
}

#[test]
fn snapshot_json_encoding_preserves_scalar_bits_object_order_and_wire_validation() {
    let value = Json::Object(
        vec![
            ("z".into(), Json::NumberBits(f64::NAN.to_bits() | 17)),
            (
                "a".into(),
                Json::Array(
                    vec![
                        Json::Null,
                        Json::Bool(true),
                        Json::NumberBits((-0.0f64).to_bits()),
                        Json::String("\"\\\n\t文本".into()),
                    ]
                    .into(),
                ),
            ),
            ("z".into(), Json::Bool(false)),
        ]
        .into(),
    );
    let snapshot = ValueSnapshot::Json(value.clone());
    let wire = encode(Node::Value(&snapshot));
    let runtime = InterpValue::Json(value);
    assert_eq!(wire, codec::value_json(&runtime));
    assert_eq!(codec::value_from_json(&wire).unwrap(), runtime);
    assert_eq!(wire["value"]["entries"][0]["key"], "z");
    assert_eq!(wire["value"]["entries"][2]["key"], "z");
    let mut invalid = wire.clone();
    invalid["value"]["entries"][0]["value"]["value"] = json!(-1);
    assert!(codec::value_from_json(&invalid).is_err());
    invalid = wire.clone();
    invalid["value"]["entries"][1]["value"]["kind"] = json!("unknown");
    assert!(codec::value_from_json(&invalid).is_err());
    invalid = wire;
    invalid["value"]["entries"][1]
        .as_object_mut()
        .unwrap()
        .remove("key");
    assert!(codec::value_from_json(&invalid).is_err());
}

#[test]
fn snapshot_json_encoding_and_file_roundtrip_are_stack_safe_and_linear() {
    for depth in [1000, 4000, 30_000] {
        let value = ValueSnapshot::Json(chain(depth));
        let (wire, cost) = measure(|| CheckpointDocument::from_value(encode(Node::Value(&value))));
        assert!(
            cost.bytes <= depth * 1536 + 4096,
            "copied descendants: {cost:?}"
        );
        assert!(
            cost.count <= depth * 12 + 32,
            "nonlinear allocation count: {cost:?}"
        );
        let mut artifact =
            json!({"schema":crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA,"checkpoint":null});
        artifact["checkpoint"] = wire.into_value();
        let limits = codec::CheckpointFileLimits::default();
        let bytes = codec::checkpoint_file_to_bytes(artifact, limits).unwrap();
        let decoded = codec::checkpoint_file_from_bytes(&bytes, limits).unwrap();
        let mut cursor = &decoded["checkpoint"]["value"];
        for _ in 0..depth {
            assert_eq!(cursor["kind"], "object");
            assert_eq!(cursor["entries"].as_array().unwrap().len(), 1);
            assert_eq!(cursor["entries"][0]["key"], "child");
            cursor = &cursor["entries"][0]["value"];
        }
        assert_eq!(cursor["kind"], "string");
        assert_eq!(cursor["value"].as_str().unwrap().len(), 1024);
        assert_eq!(
            codec::checkpoint_file_to_bytes(decoded.into_value(), limits).unwrap(),
            bytes
        );
        eprintln!("snapshot JSON depth={depth}: {cost:?}");
    }
}

#[test]
fn snapshot_json_encoding_charges_every_shared_occurrence_and_releases_partial_graphs() {
    let mut json = Json::Null;
    for _ in 0..12 {
        json = Json::Array(vec![json.clone(), json].into());
    }
    let saved = ValueSnapshot::Json(json);
    let (_, cost) = measure(|| {
        let error =
            encode_with_budget(Node::Value(&saved), &mut EncodingBudget::new(128)).unwrap_err();
        assert_eq!(
            error.message(),
            "checkpoint snapshot expansion exceeds node budget"
        );
    });
    assert_eq!(cost.bytes, cost.released_bytes);
    assert!(cost.bytes < 128 * 4096, "expanded past budget: {cost:?}");

    let deep = ValueSnapshot::Json(chain(30_000));
    let (_, cost) = measure(|| {
        assert!(encode_with_budget(Node::Value(&deep), &mut EncodingBudget::new(20_000)).is_err());
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "partial deep JSON leaked: {cost:?}"
    );
}

#[test]
fn snapshot_json_encoding_rejects_wide_arrays_and_keys_before_copying() {
    let cases = [
        ValueSnapshot::Json(Json::Array(vec![Json::Null; 100_000].into())),
        ValueSnapshot::Json(Json::Object(vec![("k".repeat(100_000), Json::Null)].into())),
        ValueSnapshot::Json(Json::String("v".repeat(100_000).into())),
        ValueSnapshot::String("v".repeat(100_000).into()),
    ];
    for value in cases {
        let (_, cost) = measure(|| {
            let mut budget = EncodingBudget::with_limits(codec::CheckpointFileLimits {
                max_nodes: 32,
                max_bytes: 32,
            });
            assert!(encode_with_budget(Node::Value(&value), &mut budget).is_err());
        });
        assert!(cost.bytes < 4096, "copied rejected payload: {cost:?}");
        assert_eq!(cost.bytes, cost.released_bytes);
    }
}

#[test]
fn snapshot_json_encoding_byte_budget_is_cumulative_even_for_shared_strings() {
    let payload = Json::String("s".repeat(1024).into());
    let value = ValueSnapshot::Json(Json::Array(vec![payload.clone(), payload].into()));
    let (_, cost) = measure(|| {
        let mut budget = EncodingBudget::with_limits(codec::CheckpointFileLimits {
            max_nodes: 16,
            max_bytes: 2047,
        });
        let error = encode_with_budget(Node::Value(&value), &mut budget).unwrap_err();
        assert_eq!(
            error.message(),
            "checkpoint snapshot payload exceeds byte budget"
        );
    });
    assert_eq!(cost.bytes, cost.released_bytes);
    let mut budget = EncodingBudget::with_limits(codec::CheckpointFileLimits {
        max_nodes: 16,
        max_bytes: 2048,
    });
    let wire = encode_with_budget(Node::Value(&value), &mut budget).unwrap();
    assert_eq!(
        wire["value"]["values"][0]["value"],
        wire["value"]["values"][1]["value"]
    );
}

#[test]
fn snapshot_json_deep_runtime_restore_is_stack_safe() {
    const WORKER: &str = "ETAS_TEST_CHECKPOINT_JSON_RESTORE_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let current = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "JSON restore subprocess failed: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    for depth in [1000, 4000, 30_000] {
        let saved = ValueSnapshot::Json(chain(depth));
        let wire = CheckpointDocument::from_value(encode(Node::Value(&saved)));
        let (restored, cost) = measure(|| codec::value_from_json(&wire).unwrap());
        let recaptured = ValueSnapshot::capture(&restored).unwrap();
        assert!(recaptured == saved);
        assert!(
            cost.bytes <= depth * 768 + 4096,
            "copied restored graph: {cost:?}"
        );
        assert!(cost.count <= depth * 3 + 32, "nonlinear restore: {cost:?}");
        eprintln!("snapshot JSON restore depth={depth}: {cost:?}");
    }
}

#[test]
fn snapshot_json_restore_rejects_late_invalid_nodes_without_leaking_deep_prefixes() {
    let invalid = [
        (json!({"kind":"bool","value":"true"}), "value"),
        (json!({"kind":"number_bits","value":-1}), "value"),
        (json!({"kind":"string","value":true}), "value"),
        (
            json!({"kind":"unknown"}),
            "unsupported host json support value",
        ),
        (
            json!({"kind":"object","entries":[{"value":{"kind":"unknown"}}]}),
            "key",
        ),
        (json!({"kind":"object","entries":[{"key":"x"}]}), "value"),
    ];
    for (invalid, expected) in invalid {
        let saved = ValueSnapshot::Json(chain(4000));
        let mut deep = CheckpointDocument::from_value(encode(Node::Value(&saved)));
        let mut wire = CheckpointDocument::from_value(
            json!({"kind":"json","value":{"kind":"array","values":[]}}),
        );
        let values = wire.value_mut()["value"]["values"].as_array_mut().unwrap();
        values.push(std::mem::take(&mut deep.value_mut()["value"]));
        values.push(invalid);
        let (_, cost) = measure(|| {
            let error = codec::value_from_json(&wire).unwrap_err();
            assert!(error.message().contains(expected), "{error}");
        });
        assert_eq!(
            cost.bytes, cost.released_bytes,
            "decoded prefix leaked: {cost:?}"
        );
    }
}

#[test]
fn snapshot_json_restore_uses_one_owned_child_buffer_per_container() {
    for count in [1000, 2000, 4000] {
        let saved = ValueSnapshot::Json(Json::Array(vec![Json::Bool(true); count].into()));
        let wire = CheckpointDocument::from_value(encode(Node::Value(&saved)));
        let (value, cost) = measure(|| codec::value_from_json(&wire).unwrap());
        assert_eq!(
            cost.count, 3,
            "expected one child Vec, one owner and one frame stack: {cost:?}"
        );
        assert!(
            cost.bytes <= count * size_of::<Json>() + 4096,
            "extra child graph: {cost:?}"
        );
        assert!(ValueSnapshot::capture(&value).unwrap() == saved);
        eprintln!("wide JSON restore n={count}: {cost:?}");
    }
}

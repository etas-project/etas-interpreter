use crate::api::codec::{self, CheckpointDocument};
use crate::testing::allocation::measure;
use crate::value::{HostJsonSupportValue as Json, InterpValue};
use serde_json::{Value, json};

// Previous production algorithm. Only bounded test inputs use this baseline.
pub(in crate::api::codec) fn legacy_wrapped(value: &Json) -> Value {
    json!({"kind":"json","value":legacy(value)})
}

fn legacy(value: &Json) -> Value {
    match value {
        Json::Null => json!({"kind":"null"}),
        Json::Bool(value) => json!({"kind":"bool","value":value}),
        Json::NumberBits(value) => json!({"kind":"number_bits","value":value}),
        Json::String(value) => json!({"kind":"string","value":value}),
        Json::Array(values) => {
            json!({"kind":"array","values":values.iter().map(legacy).collect::<Vec<_>>()})
        }
        Json::Object(entries) => {
            json!({"kind":"object","entries":entries.iter().map(|(key,value)| json!({"key":key,"value":legacy(value)})).collect::<Vec<_>>()})
        }
    }
}

fn chain(depth: usize) -> Json {
    let mut value = Json::String("x".repeat(1024).into());
    for _ in 0..depth {
        value = Json::Object(vec![("child".into(), value)].into());
    }
    value
}

#[test]
fn runtime_json_encoding_avoids_repeated_descendant_copies() {
    for depth in [16, 32, 64] {
        let value = InterpValue::Json(chain(depth));
        let (wire, cost) = measure(|| CheckpointDocument::from_value(codec::value_json(&value)));
        assert!(cost.count <= depth * 12 + 32, "descendant copies: {cost:?}");
        assert!(
            cost.bytes <= depth * 1536 + 4096,
            "descendant copies: {cost:?}"
        );
        assert_eq!(codec::value_from_json(&wire).unwrap(), value);
        eprintln!("runtime JSON encode depth={depth}: {cost:?}");
    }
}

#[test]
fn runtime_json_encoding_is_stack_safe() {
    const WORKER: &str = "ETAS_TEST_RUNTIME_JSON_CODEC_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let current = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "runtime JSON subprocess failed: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    for depth in [1000, 4000, 30_000] {
        let value = InterpValue::Json(chain(depth));
        let (wire, cost) = measure(|| CheckpointDocument::from_value(codec::value_json(&value)));
        assert_eq!(codec::value_from_json(&wire).unwrap(), value);
        assert!(cost.count <= depth * 12 + 32, "descendant copies: {cost:?}");
        assert!(
            cost.bytes <= depth * 1536 + 4096,
            "descendant copies: {cost:?}"
        );
        eprintln!("runtime JSON encode depth={depth}: {cost:?}");
    }
}

#[test]
fn all_json_encoding_boundaries_preserve_wire_bits_labels_and_order() {
    use crate::value::HostSupportValue;
    use etas_host::{HostJsonValue as HostJson, HostValue};
    let bits = [
        0.0f64.to_bits(),
        (-0.0f64).to_bits(),
        f64::INFINITY.to_bits(),
        0x7ff8_0000_0000_0007,
    ];
    let shared = Json::Object(
        vec![
            (
                "z".into(),
                Json::Array(bits.iter().map(|bits| Json::NumberBits(*bits)).collect()),
            ),
            ("a".into(), Json::String("\"\\\n\t".into())),
            ("z".into(), Json::Null),
        ]
        .into(),
    );
    let host = HostJson::Object(vec![
        (
            "z".into(),
            HostJson::Array(
                bits.iter()
                    .map(|bits| HostJson::Number(f64::from_bits(*bits)))
                    .collect(),
            ),
        ),
        ("a".into(), HostJson::String("\"\\\n\t".into())),
        ("z".into(), HostJson::Null),
    ]);
    let expected = legacy_wrapped(&shared);
    let runtime = codec::value_json(&InterpValue::Json(shared.clone()));
    let support = codec::value::host_support_value_json(&HostSupportValue::Json(shared.clone()));
    let external = codec::host_value_json(&HostValue::Json(host));
    assert_eq!(runtime, expected);
    assert_eq!(support, expected);
    assert_eq!(external, expected);
    let decoded_host = codec::host_value_from_json(&external).unwrap();
    assert_eq!(codec::host_value_json(&decoded_host), expected);
    assert_eq!(
        codec::value_from_json(&runtime).unwrap(),
        InterpValue::Json(shared)
    );
}

#[test]
fn host_and_support_json_encoders_remove_intermediate_payload_copies() {
    use crate::value::HostSupportValue;
    use etas_host::{HostJsonValue as HostJson, HostValue};
    for count in [1000, 2000, 4000] {
        let shared = Json::Object(
            (0..count)
                .map(|n| (n.to_string(), Json::String("p".repeat(1024).into())))
                .collect(),
        );
        let host = HostValue::Json(HostJson::Object(
            (0..count)
                .map(|n| (n.to_string(), HostJson::String("p".repeat(1024))))
                .collect(),
        ));
        let support = HostSupportValue::Json(shared.clone());
        let (expected, old) = measure(|| CheckpointDocument::from_value(legacy_wrapped(&shared)));
        let (external, host_cost) =
            measure(|| CheckpointDocument::from_value(codec::host_value_json(&host)));
        let (internal, support_cost) = measure(|| {
            CheckpointDocument::from_value(codec::value::host_support_value_json(&support))
        });
        assert_eq!(*external, *expected);
        assert_eq!(*internal, *expected);
        for cost in [host_cost, support_cost] {
            assert!(
                old.bytes > cost.bytes + count * 1024,
                "retained intermediate payload copy: old={old:?}, new={cost:?}"
            );
        }
        eprintln!(
            "JSON boundary n={count}: old={old:?}, host={host_cost:?}, support={support_cost:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn deep_json_report_checkpoint_arguments_and_resume_keep_the_same_value() {
    use crate::api::{EntryPoint, RunOptions};
    use crate::orchestration::{
        ContinuationSnapshot, MachineFrameSnapshot, ValueSnapshot, WorkflowEvent,
    };
    use crate::testing::{FakeHost, project::checked_project};
    let checked = checked_project(
        r#"
module app.main;
import std.json.JsonValue;
import std.runtime.checkpoint;
flow main(data: JsonValue) -> JsonValue {
    checkpoint("json-argument");
    return data;
}
"#,
    );
    let host = FakeHost::new(crate::host::HostServiceAvailability::with_host(
        etas_effects::HostRequirementKind::Checkpoint,
    ));
    let input = InterpValue::Json(chain(4000));
    let mut result = crate::Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![input.clone()],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&input));
    result.events.push(WorkflowEvent::MessageCreated {
        id: "message".into(),
        from: None,
        to: None,
        session: None,
        role: "user".into(),
        created_at: "step-1".into(),
        payload: Box::new(input.clone()),
        provenance: None,
    });
    let report = CheckpointDocument::from_value(
        codec::run_report_json("run", &[], "main", &result).unwrap(),
    );
    assert_eq!(codec::value_from_json(&report["value"]).unwrap(), input);
    assert_eq!(
        codec::value_from_json(&report["events"].as_array().unwrap().last().unwrap()["payload"])
            .unwrap(),
        input
    );
    let file = codec::checkpoint_file_to_bytes(
        codec::checkpoint_artifact_json(&[], "main", &result.checkpoints[0]).unwrap(),
        codec::CheckpointFileLimits::default(),
    )
    .unwrap();
    let document =
        codec::checkpoint_file_from_bytes(&file, codec::CheckpointFileLimits::default()).unwrap();
    let checkpoint = codec::checkpoint_from_json(&document, &checked).unwrap();
    let resumed = crate::Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), Some(&input));

    // Reject after a prior deep checkpoint and report value were materialized.
    // The oversized byte leaf fails admission before allocating its JSON array.
    let mut invalid = checkpoint.clone();
    invalid.machine.frames = vec![MachineFrameSnapshot::Continuation {
        continuation: ContinuationSnapshot::ListConsTail {
            head: ValueSnapshot::Bytes(vec![0; 1_000_000].into()),
            span: checked.hir.blocks.iter().next().unwrap().1.span,
        },
    }];
    result.checkpoints.push(invalid);
    let (_, cost) = measure(|| {
        let error = codec::run_report_json("run", &[], "main", &result).unwrap_err();
        assert!(error.message().contains("node budget"));
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "partial report leaked: {cost:?}"
    );
}

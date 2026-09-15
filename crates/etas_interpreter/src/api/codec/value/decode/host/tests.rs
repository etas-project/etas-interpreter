use crate::api::codec::{self, CheckpointDocument};
use crate::testing::allocation::measure;
use crate::value::HostSupportValue as H;

fn model(payload: H) -> crate::value::InterpValue {
    use crate::value::{ModelContentValue, ModelMessageValue, ModelResponseValue, ModelRoleValue};
    crate::value::InterpValue::ModelResponse(ModelResponseValue {
        id: 1,
        message: ModelMessageValue {
            role: ModelRoleValue::Assistant,
            content: vec![ModelContentValue::Value(payload)],
        },
        tool_calls: vec![],
        usage: None,
    })
}

fn encode(payload: &H) -> CheckpointDocument {
    CheckpointDocument::from_value(codec::value_json(&model(payload.clone())))
}

fn decode(wire: &serde_json::Value) -> Result<H, codec::InterpreterCodecError> {
    let crate::value::InterpValue::ModelResponse(mut response) = codec::value_from_json(wire)?
    else {
        panic!("model response");
    };
    let Some(crate::value::ModelContentValue::Value(value)) = response.message.content.pop() else {
        panic!("model payload");
    };
    Ok(value)
}

fn chain(depth: usize) -> H {
    (0..depth).fold(H::Bytes(vec![7; 4096].into()), |value, i| match i % 4 {
        0 => H::List(vec![value].into()),
        1 => H::Map(vec![(H::String("key".into()), value)].into()),
        2 => H::Record(vec![("field".into(), value)].into()),
        _ => H::Variant {
            name: "Payload".into(),
            fields: vec![value].into(),
        },
    })
}

#[test]
fn host_payload_aliases_do_not_copy_descendants() {
    for depth in [16, 32, 64] {
        let original = chain(depth);
        let (alias, cost) = measure(|| original.clone());
        eprintln!("host payload clone depth={depth}: {cost:?}");
        assert_eq!(cost.count, 0, "{cost:?}");
        assert_eq!(cost.bytes, 0, "{cost:?}");
        let (equal, compared) = measure(|| alias == original);
        assert!(equal);
        assert_eq!(
            compared.count, 0,
            "shared payload comparison allocated: {compared:?}"
        );
    }
}

#[test]
fn host_payload_clone_compare_and_release_are_stack_safe() {
    const WORKER: &str = "ETAS_HOST_PAYLOAD_LIFECYCLE_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let thread = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", thread.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let original = chain(30_000);
    let alias = original.clone();
    let independent = chain(30_000);
    assert_eq!(alias, independent);
    let wire = encode(&alias);
    let decoded = decode(&wire).unwrap();
    assert_eq!(decoded, alias);
    drop(original);
    assert_eq!(alias, independent);
    drop(alias);
    drop(independent);
}

#[test]
fn host_payload_decode_releases_deep_partial_results_on_error() {
    let payload = chain(30_000);
    let mut wire = encode(&H::Map(vec![(payload, H::Bool(true))].into()));
    wire.value_mut()["message"]["content"][0]["value"]["entries"][0]["value"]["value"] =
        serde_json::json!("not-a-bool");
    let (_, cost) = measure(|| {
        let error = decode(&wire).unwrap_err();
        assert!(error.message().contains("bool"), "{}", error.message());
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "partial decoded payload leaked: {cost:?}"
    );
}

#[test]
fn host_payload_shared_branch_release_does_not_leak_or_destroy_retained_children() {
    let (_, cost) = measure(|| {
        let retained = chain(1000);
        let tree = (0..32).fold(retained.clone(), |child, _| {
            H::Map(vec![(child.clone(), child), (H::Unit, retained.clone())].into())
        });
        drop(tree);
        assert_eq!(retained, chain(1000));
        drop(retained);
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "shared Host payload leaked: {cost:?}"
    );
}

#[test]
fn host_owned_ingress_moves_text_and_bytes_and_handles_deep_payloads() {
    use etas_host::{HostJsonValue as J, HostValue as V};
    let text = "x".repeat(4096);
    let text_pointer = text.as_ptr();
    let bytes = vec![7; 4096];
    let bytes_pointer = bytes.as_ptr();
    let value = V::Record(vec![
        ("text".into(), V::String(text)),
        ("bytes".into(), V::Bytes(bytes)),
    ]);
    let value = H::from(value);
    let H::Record(fields) = &value else {
        panic!("record");
    };
    let H::String(text) = &fields[0].1 else {
        panic!("text");
    };
    let H::Bytes(bytes) = &fields[1].1 else {
        panic!("bytes");
    };
    assert_eq!(text.as_ptr(), text_pointer);
    assert_eq!(bytes.as_ptr(), bytes_pointer);
    let input = (0..30_000).fold(V::Bool(true), |child, i| match i % 4 {
        0 => V::List(vec![child]),
        1 => V::Map(vec![(V::Unit, child)]),
        2 => V::Record(vec![("field".into(), child)]),
        _ => V::Variant {
            name: "Payload".into(),
            fields: vec![child],
        },
    });
    let output = H::from(input);
    let wire = encode(&output);
    assert_eq!(decode(&wire).unwrap(), output);
    let json = (0..30_000).fold(J::Null, |child, _| J::Array(vec![child]));
    let json = H::from(V::Json(json));
    let alias = json.clone();
    drop(json);
    drop(alias);
}

#[test]
fn model_checkpoint_capture_clone_restore_cost_is_independent_of_payload_size() {
    use crate::value::{ModelContentValue, ModelMessageValue, ModelResponseValue, ModelRoleValue};
    use crate::{orchestration::ValueSnapshot, value::InterpValue};
    let mut previous = None;
    for depth in [1000, 2000, 4000] {
        let mut value = InterpValue::ModelResponse(ModelResponseValue {
            id: 1,
            message: ModelMessageValue {
                role: ModelRoleValue::Assistant,
                content: vec![ModelContentValue::Value(chain(depth))],
            },
            tool_calls: vec![],
            usage: None,
        });
        let ((saved, restored), cost) = measure(|| {
            let saved = ValueSnapshot::capture(&value).unwrap();
            let copy = saved.clone();
            (saved, copy.restore().unwrap())
        });
        eprintln!("model snapshot depth={depth}: {cost:?}");
        assert!(cost.count < 10 && cost.bytes < 2048, "{cost:?}");
        if let Some(previous) = previous {
            assert_eq!((cost.count, cost.bytes), previous);
        }
        previous = Some((cost.count, cost.bytes));
        assert_eq!(restored, value);
        let InterpValue::ModelResponse(response) = &mut value else {
            unreachable!()
        };
        response.message.content[0] = ModelContentValue::Value(H::Unit);
        assert_ne!(value, restored);
        assert_eq!(saved.restore().unwrap(), restored);
    }
}

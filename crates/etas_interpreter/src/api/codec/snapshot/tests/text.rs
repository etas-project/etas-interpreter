use super::*;
use etas_builtin::text::input::TextInput;
use std::fmt::Write as _;

fn cold(prefix: &str, suffix: &str) -> StringValue {
    let prefix = StringValue::from(prefix);
    let alias = prefix.clone();
    let value = prefix.concat(suffix.into()).unwrap();
    drop(alias);
    assert!(value.contiguous().is_none());
    value
}

#[derive(Debug, PartialEq)]
enum Encoded {
    Json(Value),
    Bytes(Vec<u8>),
    Text(String),
}

fn encoded(mode: usize, text: &StringValue, bytes: &mut Vec<u8>, output: &mut String) -> Encoded {
    match mode {
        0 => Encoded::Json(serde_json::to_value(text).unwrap()),
        1 => Encoded::Json(
            codec::value_json(&InterpValue::String(text.clone()))
                .unwrap()
                .into_value(),
        ),
        2 => Encoded::Json(encode(Node::Value(&ValueSnapshot::String(text.clone())))),
        3 => Encoded::Json(
            codec::value_json(&InterpValue::Json(HostJsonSupportValue::String(
                text.clone(),
            )))
            .unwrap()
            .into_value(),
        ),
        4 => Encoded::Json(encode(Node::Value(&ValueSnapshot::Json(
            HostJsonSupportValue::String(text.clone()),
        )))),
        5 => Encoded::Json(codec::value::encode_host::encode(
            &HostSupportValue::String(text.clone()),
        )),
        6 => {
            serde_json::to_writer(&mut *bytes, text).unwrap();
            Encoded::Bytes(std::mem::take(bytes))
        }
        7 => {
            write!(output, "{text}").unwrap();
            Encoded::Text(std::mem::take(output))
        }
        8 => {
            write!(output, "{text:?}").unwrap();
            Encoded::Text(std::mem::take(output))
        }
        9 | 10 => {
            let value = InterpValue::Prompt(
                vec![PromptMessage {
                    role: PromptRole::User,
                    text: text.clone(),
                    trust: Some(TrustWrapper::Untrusted),
                }]
                .into(),
            );
            if mode == 9 {
                Encoded::Json(codec::value_json(&value).unwrap().into_value())
            } else {
                let snapshot = ValueSnapshot::capture(&value).unwrap();
                Encoded::Json(encode(Node::Value(&snapshot)))
            }
        }
        11 | 12 => {
            let value = InterpValue::Variant {
                name: text.clone(),
                fields: vec![InterpValue::Unit].into(),
            };
            if mode == 11 {
                Encoded::Json(codec::value_json(&value).unwrap().into_value())
            } else {
                let snapshot = ValueSnapshot::capture(&value).unwrap();
                Encoded::Json(encode(Node::Value(&snapshot)))
            }
        }
        13 => Encoded::Json(codec::value::encode_host::encode(
            &HostSupportValue::Variant {
                name: text.clone(),
                fields: vec![HostSupportValue::Unit].into(),
            },
        )),
        14 => Encoded::Json(codec::value::encode_host::encode(&HostSupportValue::Json(
            HostJsonSupportValue::Object(
                vec![("nested".into(), HostJsonSupportValue::String(text.clone()))].into(),
            ),
        ))),
        _ => unreachable!(),
    }
}

#[test]
fn cold_text_codec_and_formatting_do_not_retain_a_flat_input_cache() {
    let mut costs = Vec::new();
    for count in [1000, 2000, 4000] {
        let prefix = "中😀\n\"\\\0payload".repeat(count);
        let expected = format!("{prefix}tail");
        let wire = serde_json::to_vec(&expected).unwrap();
        let rendered = format!("{expected:?}");
        for mode in 0..15 {
            let text = cold(&prefix, "tail");
            let flat = StringValue::from(expected.clone());
            let mut bytes = Vec::with_capacity(wire.len());
            let mut output = String::with_capacity(rendered.len());
            let (actual, cost) = measure(|| encoded(mode, &text, &mut bytes, &mut output));
            let cached = text.contiguous().is_some();
            let mut bytes = Vec::with_capacity(wire.len());
            let mut output = String::with_capacity(rendered.len());
            let (reference, flat_cost) = measure(|| encoded(mode, &flat, &mut bytes, &mut output));
            assert_eq!(actual, reference, "mode={mode}, n={count}");
            if (9..=10).contains(&mode) {
                assert!(
                    cost.bytes <= expected.len() + 4096,
                    "Prompt encoder copied an intermediate payload: mode={mode}, n={count}: {cost:?}"
                );
            }
            costs.push((count, mode, cost, flat_cost, cached));
        }
    }
    eprintln!("cold text codec/format costs: {costs:?}");
    for (count, mode, cost, flat_cost, cached) in costs {
        assert!(
            !cached,
            "mode={mode}, n={count}: retained flat input, {cost:?}"
        );
        if (6..=8).contains(&mode) {
            assert_eq!(
                cost.count, 0,
                "preallocated writer mode={mode}, n={count}: {cost:?}"
            );
        } else if mode != 0 {
            assert_eq!(
                (cost.count, cost.bytes, cost.released_bytes),
                (flat_cost.count, flat_cost.bytes, flat_cost.released_bytes),
                "codec mode={mode}, n={count}"
            );
        }
    }
}

#[test]
fn cold_text_streaming_errors_do_not_materialize_or_consume_the_input() {
    struct StoppedWriter(usize);
    impl std::io::Write for StoppedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0 == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "test writer stopped",
                ));
            }
            let written = bytes.len().min(self.0);
            self.0 -= written;
            Ok(written)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let input = "sensitive-test-marker中😀".repeat(4000);
    let text = cold(&input, "tail");
    let (failure, cost) = measure(|| serde_json::to_writer(StoppedWriter(16), &text).unwrap_err());
    assert_eq!(
        failure.io_error_kind(),
        Some(std::io::ErrorKind::BrokenPipe)
    );
    assert!(!failure.to_string().contains("sensitive-test-marker"));
    assert!(
        text.contiguous().is_none(),
        "failed output cached input: {cost:?}"
    );
    assert!(cost.bytes < 4096, "failed output copied input: {cost:?}");
    assert_eq!(text, format!("{input}tail"));
}

#[test]
fn cold_text_checkpoint_budget_rejects_before_serializing_an_expanded_dag() {
    let mut text = StringValue::from("x");
    for _ in 0..32 {
        text = text.clone().concat(text).unwrap();
    }
    for snapshot in [
        ValueSnapshot::String(text.clone()),
        ValueSnapshot::Json(HostJsonSupportValue::String(text.clone())),
    ] {
        let mut budget = EncodingBudget::with_limits(codec::CheckpointFileLimits {
            max_bytes: 64,
            ..Default::default()
        });
        let (failure, cost) =
            measure(|| encode_with_budget(Node::Value(&snapshot), &mut budget).unwrap_err());
        assert!(failure.to_string().contains("budget"));
        assert!(text.contiguous().is_none());
        assert!(cost.bytes < 4096, "rejection expanded input: {cost:?}");
    }
}

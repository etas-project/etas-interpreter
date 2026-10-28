use super::*;
use crate::testing::allocation::measure;
use serde_json::json;

#[test]
fn document_writer_matches_canonical_json_without_reencoding_descendants() {
    for value in [
        json!(null),
        json!(true),
        json!(-0.0),
        json!(u64::MAX),
        json!("quote\" slash\\ newline\n non-ascii: 中文"),
        json!([]),
        json!({}),
        json!({"z":[null,{"escaped\nkey":"text", "n":-7}], "a":[[],{}]}),
    ] {
        let expected = serde_json::to_vec(&value).unwrap();
        let document = JsonDocument::from(value);
        let mut bytes = Vec::new();
        document.write_to(&mut bytes).unwrap();
        assert_eq!(bytes, expected);
        assert_eq!(document.to_string().as_bytes(), expected);
        assert_eq!(document, document.clone());
    }
}

#[test]
fn document_writer_borrows_wide_values_and_preserves_sink_errors() {
    for width in [1000, 2000, 4000] {
        let document = JsonDocument::from(Value::Array(
            (0..width)
                .map(|_| json!({"payload":"x".repeat(1024)}))
                .collect(),
        ));
        let (_, cost) = measure(|| document.write_to(io::sink()).unwrap());
        eprintln!("JSON document borrowed write width={width}: {cost:?}");
        assert!(cost.count <= 8, "{cost:?}");
        assert!(cost.bytes <= 2048, "{cost:?}");
        let (bytes, serialized) = measure(|| {
            let mut bytes = Vec::new();
            document.write_to(&mut bytes).unwrap();
            bytes
        });
        assert!(bytes.len() >= width * 1024);
        assert!(serialized.bytes <= bytes.len() * 4 + 2048, "{serialized:?}");
        eprintln!(
            "JSON document output width={width}: bytes={} cost={serialized:?}",
            bytes.len()
        );
    }
    struct Limited(usize);
    impl io::Write for Limited {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let document = JsonDocument::from(json!({"value":["escaped\ntext", 12, false]}));
    let text = document.to_string();
    for budget in 0..text.len() {
        let error = document.write_to(Limited(budget)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
    document.write_to(Limited(text.len())).unwrap();
    assert_eq!(document.to_string(), text);
}

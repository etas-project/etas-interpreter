use super::*;
use crate::testing::allocation::measure;

fn segmented(prefix: &str, suffix: &str) -> StringValue {
    let prefix = StringValue::from(prefix);
    let alias = prefix.clone();
    let result = prefix.concat(suffix.into()).unwrap();
    drop(alias);
    assert!(result.contiguous().is_none());
    result
}

#[test]
fn cold_text_debug_and_json_match_native_escaping_for_every_unicode_scalar() {
    let source: String = (0..=0x10ffff).filter_map(char::from_u32).collect();
    let middle = source
        .char_indices()
        .map(|(offset, _)| offset)
        .find(|offset| *offset >= source.len() / 2)
        .unwrap();
    let text = segmented(&source[..middle], &source[middle..]);
    assert_eq!(format!("{text:?}"), format!("{source:?}"));
    assert_eq!(
        serde_json::to_vec(&text).unwrap(),
        serde_json::to_vec(&source).unwrap()
    );
    assert!(text.contiguous().is_none());
}

#[test]
fn cold_text_formatting_preserves_existing_flags_and_chunk_boundary_semantics() {
    let source = "'\"\\\0\t\r\n\u{7f}e\u{301}中😀\u{200d}\u{2028}";
    for middle in source.char_indices().skip(1).map(|(offset, _)| offset) {
        let text = segmented(&source[..middle], &source[middle..]);
        assert_eq!(format!("{text:#?}"), format!("{source:#?}"));
        assert_eq!(format!("{text:>40.3?}"), format!("{source:>40.3?}"));
        // StringValue's existing Display writes raw text, ignoring format flags.
        assert_eq!(format!("{text:>40.3}"), source);
        assert_eq!(
            serde_json::to_string(&text).unwrap(),
            serde_json::to_string(source).unwrap()
        );
        assert!(text.contiguous().is_none());
    }
}

#[test]
fn cold_text_formatting_walks_deep_left_right_and_shared_graphs_iteratively() {
    let piece = StringValue::from("中\n");
    let mut left = StringValue::default();
    let mut right = StringValue::default();
    for _ in 0..20_000 {
        left = left.concat(piece.clone()).unwrap();
        right = piece.clone().concat(right).unwrap();
    }
    let mut shared = StringValue::from("中\n");
    for _ in 0..16 {
        shared = shared.clone().concat(shared).unwrap();
    }
    for (text, count) in [(left, 20_000), (right, 20_000), (shared, 1 << 16)] {
        let expected = "中\n".repeat(count);
        assert_eq!(format!("{text}"), expected);
        assert_eq!(format!("{text:?}"), format!("{expected:?}"));
        assert_eq!(
            serde_json::to_vec(&text).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        assert!(text.contiguous().is_none());
    }
}

#[test]
fn cold_text_formatting_errors_stop_before_expanding_a_shared_graph() {
    struct StoppedWriter(usize);
    impl fmt::Write for StoppedWriter {
        fn write_str(&mut self, value: &str) -> fmt::Result {
            self.0 = self.0.checked_sub(value.len()).ok_or(fmt::Error)?;
            Ok(())
        }
    }
    let mut text = StringValue::from("x");
    for _ in 0..30 {
        text = text.clone().concat(text).unwrap();
    }
    for debug in [false, true] {
        let (result, cost) = measure(|| {
            let mut writer = StoppedWriter(4);
            if debug {
                write!(writer, "{text:?}")
            } else {
                write!(writer, "{text}")
            }
        });
        assert!(result.is_err());
        assert!(cost.bytes < 4096, "stopped writer expanded input: {cost:?}");
        assert!(text.contiguous().is_none());
    }
}

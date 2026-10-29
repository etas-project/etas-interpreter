use super::*;
use crate::{
    control::Frame, plan::SlotLayoutTable, testing::allocation::measure, value::InterpValue,
};
use etas_hir::SymbolId;
use std::sync::Arc;

#[test]
fn borrowed_text_chunks_preserve_mixed_iteration_without_caching_payloads() {
    use etas_builtin::text::input::TextInput;
    let parts = ["first", "é", "中", "🙂", "\0", "last"];
    let mut text = StringValue::from(parts[0]);
    for part in &parts[1..] {
        text = text.clone().concat((*part).into()).unwrap();
    }
    assert!(text.contiguous().is_none());
    assert_eq!(text.chunks().collect::<Vec<_>>(), parts);
    assert_eq!(
        text.chunks().rev().collect::<Vec<_>>(),
        parts.into_iter().rev().collect::<Vec<_>>()
    );
    let mut chunks = text.chunks();
    assert_eq!(chunks.len(), parts.len());
    assert_eq!(chunks.next(), Some("first"));
    assert_eq!(chunks.next_back(), Some("last"));
    assert_eq!(chunks.next_back(), Some("\0"));
    assert_eq!(chunks.next(), Some("é"));
    assert_eq!(chunks.next(), Some("中"));
    assert_eq!(chunks.next_back(), Some("🙂"));
    assert_eq!(chunks.next(), None);
    assert_eq!(chunks.next_back(), None);
    assert!(text.contiguous().is_none());
}

#[test]
fn borrowed_text_chunks_allocate_only_cursor_metadata_for_deep_chains() {
    use etas_builtin::text::input::TextInput;
    for count in [1000, 2000, 4000] {
        let piece = StringValue::from("中😀e\u{301}".repeat(128));
        let mut text = StringValue::default();
        for _ in 0..count {
            text = text.clone().concat(piece.clone()).unwrap();
        }
        let (scalars, cost) = measure(|| text.chunks().flat_map(str::chars).count());
        assert_eq!(scalars, count * 128 * 4);
        assert!(text.contiguous().is_none());
        assert!(
            cost.bytes <= count * 64,
            "cursor metadata n={count}: {cost:?}"
        );
        eprintln!(
            "deep chunk cursor n={count}: {cost:?}, logical payload={}",
            text.len()
        );
    }
}

#[test]
fn text_reads_and_unique_extraction_preserve_the_payload_buffer() {
    for length in [1024, 2048, 4096] {
        let mut buffer = String::with_capacity(length + 16);
        buffer.push_str(&"x".repeat(length));
        let pointer = buffer.as_ptr();
        let text = StringValue::from(buffer);
        let mut frame = Frame::new(Arc::new(SlotLayoutTable::from_symbols(vec![SymbolId(0)])));
        frame.insert(SymbolId(0), InterpValue::String(text));
        let (alias, allocations) = measure(|| frame.get(SymbolId(0)).unwrap());
        assert_eq!(allocations.count, 0);
        let InterpValue::String(alias) = alias else {
            panic!("text")
        };
        assert_eq!(alias.as_ptr(), pointer);
        frame.set(SymbolId(0), InterpValue::Unit);
        let (owned, allocations) = measure(|| alias.into_string());
        assert_eq!(allocations.count, 0);
        assert_eq!(owned.as_ptr(), pointer);
        let mut unique = StringValue::from(owned);
        let (_, allocations) = measure(|| unique.push_str("suffix"));
        assert_eq!(
            allocations.count, 0,
            "unique temporary reuses spare capacity"
        );
        assert_eq!(unique.as_ptr(), pointer);

        let alias = unique.clone();
        let (_, allocations) = measure(|| unique.push_str("!"));
        assert!(
            allocations.bytes >= length,
            "live aliases require a text copy"
        );
        assert_ne!(unique.as_ptr(), alias.as_ptr());
        assert!(unique.ends_with("suffix!"));
        assert!(alias.ends_with("suffix"));
        eprintln!("shared text write n={length}: {allocations:?}");
    }
}

#[test]
fn shared_text_keeps_content_equality_and_snapshot_independence() {
    let mut text = StringValue::from("😀e\u{301}\n");
    let alias = text.clone();
    let snapshot =
        crate::orchestration::ValueSnapshot::capture(&InterpValue::String(text.clone())).unwrap();
    text.push_str("changed");
    assert_eq!(alias, StringValue::from("😀e\u{301}\n"));
    assert_eq!(snapshot.restore().unwrap(), InterpValue::String(alias));
    assert_eq!(
        serde_json::to_value(&text).unwrap(),
        serde_json::json!("😀e\u{301}\nchanged")
    );
}

#[test]
fn consuming_text_concat_reuses_unique_buffers_and_shares_live_prefixes() {
    for length in [1000, 2000, 4000] {
        let mut buffer = String::with_capacity(length + 128);
        buffer.push_str(&"x".repeat(length));
        let pointer = buffer.as_ptr();
        let text = StringValue::from(buffer);
        let suffix = StringValue::from("中😀");
        let (text, cost) = measure(|| text.concat(suffix).unwrap());
        assert_eq!(
            cost.count, 0,
            "unique concat must reuse its buffer: {cost:?}"
        );
        assert_eq!(text.as_ptr(), pointer);
        let previous = text.clone();
        let suffix = StringValue::from("!");
        let (text, cost) = measure(|| text.concat(suffix).unwrap());
        eprintln!("shared concat n={length}: {cost:?}");
        assert_eq!(cost.count, 1, "only an immutable concat node: {cost:?}");
        assert!(cost.bytes < 256, "copied the shared prefix: {cost:?}");
        let (_, read_cost) = measure(|| {
            assert_eq!(text.len(), length + "中😀!".len());
            let alias = text.clone();
            assert!(!alias.is_empty());
            assert_eq!(text, alias);
            assert_eq!(text.cmp(&alias), Ordering::Equal);
        });
        assert_eq!(read_cost.count, 0);
        let ((rendered, pointer), output_cost) = measure(|| (text.as_str().len(), text.as_ptr()));
        assert_eq!(rendered, length + "中😀!".len());
        assert!(
            output_cost.bytes >= rendered,
            "count required contiguous output separately"
        );
        assert!(
            output_cost.bytes <= rendered + 256,
            "intermediate payload copy: {output_cost:?}"
        );
        let (_, cached_cost) = measure(|| assert_eq!(text.as_ptr(), pointer));
        assert_eq!(cached_cost.count, 0);
        assert!(previous.ends_with("中😀"));
        assert!(text.ends_with("中😀!"));
        let (owned, consumed_cost) = measure(|| text.into_string());
        assert_eq!(consumed_cost.count, 0, "move the cached owned result");
        assert_eq!(owned.as_ptr(), pointer);
    }
}

#[test]
fn text_concat_versions_share_chunks_and_release_the_last_owner() {
    for count in [1000, 2000, 4000] {
        let piece = StringValue::from("中😀");
        let mut text = StringValue::default();
        let mut versions = Vec::with_capacity(count);
        let (_, cost) = measure(|| {
            for _ in 0..count {
                text = text.clone().concat(piece.clone()).unwrap();
                versions.push(text.clone());
            }
        });
        eprintln!("retained concat versions n={count}: {cost:?}");
        assert_eq!(cost.count, count - 1);
        assert!(
            cost.bytes <= count * 128,
            "retained versions copied payloads: {cost:?}"
        );
        for (index, version) in versions.iter().enumerate() {
            assert_eq!(version.len(), (index + 1) * "中😀".len());
        }
        let Repr::Concat(node) = &versions[1].0 else {
            panic!("shared prefix node")
        };
        let weak = Rc::downgrade(node);
        let value = InterpValue::String(text.clone());
        let (snapshot, capture_cost) =
            measure(|| crate::orchestration::ValueSnapshot::capture(&value).unwrap());
        assert_eq!(capture_cost.count, 0);
        let (restored, restore_cost) = measure(|| snapshot.restore().unwrap());
        assert_eq!(restore_cost.count, 0);
        drop(versions);
        drop(text);
        drop(value);
        assert!(
            weak.upgrade().is_some(),
            "restored value retains its prefixes"
        );
        let InterpValue::String(restored) = restored else {
            panic!("text")
        };
        let (output, output_cost) = measure(|| restored.into_string());
        assert_eq!(output, "中😀".repeat(count));
        assert!(
            output_cost.bytes <= output.len() * 4,
            "excessive output staging: {output_cost:?}"
        );
        assert!(
            weak.upgrade().is_none(),
            "all prefix nodes must be released"
        );
    }
}

#[test]
fn deep_text_concat_uses_iterative_materialization_and_normal_release() {
    if crate::testing::in_subprocess("ETAS_DEEP_TEXT_CONCAT_WORKER") {
        return;
    }
    let mut text = StringValue::default();
    let piece = StringValue::from("中😀");
    for _ in 0..30_000 {
        text = text.clone().concat(piece.clone()).unwrap();
    }
    let alias = text.clone();
    let expected = "中😀".repeat(30_000);
    assert_eq!(
        serde_json::to_string(&text).unwrap(),
        serde_json::to_string(&expected).unwrap()
    );
    assert_eq!(text.as_str(), expected);
    let mut left = std::collections::hash_map::DefaultHasher::new();
    let mut right = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut left);
    expected.hash(&mut right);
    assert_eq!(left.finish(), right.finish());
    drop(text);
    assert_eq!(alias.into_string(), expected);
    let mut text = StringValue::default();
    for _ in 0..30_000 {
        text = text.clone().concat(piece.clone()).unwrap();
    }
    drop(text);
}

#[test]
fn concat_preserves_empty_unicode_rhs_and_materialized_aliases() {
    let prefix = StringValue::from("e");
    let suffix = prefix
        .clone()
        .concat(StringValue::from("\u{301}😀"))
        .unwrap();
    let suffix_alias = suffix.clone();
    let buffer = String::with_capacity(128);
    let pointer = buffer.as_ptr();
    let text = StringValue::from(buffer).concat(suffix).unwrap();
    assert_eq!(
        text.as_ptr(),
        pointer,
        "the empty unique destination still owns capacity"
    );
    assert_eq!(text.as_str(), "e\u{301}😀");
    assert_eq!(suffix_alias.as_str(), text.as_str());
    let alias = text.clone();
    let text = text.concat(StringValue::from("中")).unwrap();
    assert_eq!(text.as_str(), "e\u{301}😀中");
    let materialized = text.clone();
    let text = text.concat(StringValue::from("!")).unwrap();
    let (text, empty_cost) = measure(|| text.concat(StringValue::default()).unwrap());
    assert_eq!(empty_cost.count, 0);
    assert_eq!(alias.as_str(), "e\u{301}😀");
    assert_eq!(materialized.as_str(), "e\u{301}😀中");
    assert_eq!(text.into_string(), "e\u{301}😀中!");
    assert_eq!(prefix.as_str(), "e");
}

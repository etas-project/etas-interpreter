use super::*;
use crate::{
    control::Frame, plan::SlotLayoutTable, testing::allocation::measure, value::InterpValue,
};
use etas_hir::SymbolId;
use std::sync::Arc;

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

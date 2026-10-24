use super::*;
use crate::{
    control::Frame, plan::SlotLayoutTable, testing::allocation::measure, value::InterpValue,
};
use etas_hir::SymbolId;
use std::sync::Arc;

#[test]
fn byte_reads_share_backing_and_owned_extraction_copies_only_live_aliases() {
    for length in [1000, 2000, 4000] {
        let buffer = vec![7; length];
        let pointer = buffer.as_ptr();
        let value = BytesValue::from(buffer);
        assert_eq!(value.as_ptr(), pointer);
        let mut frame = Frame::new(Arc::new(SlotLayoutTable::from_symbols(vec![SymbolId(0)])));
        frame.insert(SymbolId(0), InterpValue::Bytes(value));
        let (alias, cost) = measure(|| frame.get(SymbolId(0)).unwrap());
        assert_eq!(cost.count, 0);
        assert_eq!(cost.bytes, 0);
        let InterpValue::Bytes(alias) = alias else {
            panic!("bytes")
        };
        assert_eq!(alias.as_ptr(), pointer);

        let (mut owned, cost) = measure(|| alias.clone().into_vec());
        assert_eq!(cost.count, 1);
        assert_eq!(
            cost.bytes, length,
            "a live alias requires one owned payload copy"
        );
        owned[0] = 9;
        assert_eq!(alias[0], 7);
        assert_ne!(owned.as_ptr(), pointer);

        frame.set(SymbolId(0), InterpValue::Unit);
        let (owned, cost) = measure(|| alias.into_vec());
        assert_eq!(cost.count, 0);
        assert_eq!(cost.bytes, 0);
        assert_eq!(owned.as_ptr(), pointer);
    }
}

#[test]
fn byte_snapshot_is_independent_and_preserves_its_wire_value() {
    let value = BytesValue::from(vec![0, 128, 255]);
    assert_eq!(format!("{value:?}"), "[0, 128, 255]");
    let snapshot =
        crate::orchestration::ValueSnapshot::capture(&InterpValue::Bytes(value.clone())).unwrap();
    let mut owned = value.into_vec();
    owned[0] = 42;
    let restored = snapshot.restore().unwrap();
    assert_eq!(restored, InterpValue::Bytes(vec![0, 128, 255].into()));
    assert_eq!(
        crate::api::codec::value_json(&restored),
        serde_json::json!({"kind":"bytes", "value":[0,128,255]}),
    );
}

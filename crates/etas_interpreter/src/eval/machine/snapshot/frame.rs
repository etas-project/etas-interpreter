use crate::control::Frame;
use crate::orchestration::LocalsSnapshot;

pub(super) fn capture_frame(frame: &Frame) -> Result<LocalsSnapshot, String> {
    Ok(LocalsSnapshot {
        locals: frame
            .sorted_locals()
            .into_iter()
            .map(|(symbol, value)| {
                Ok((
                    symbol,
                    crate::orchestration::ValueSnapshot::capture(&value)?,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?,
        type_bindings: frame.sorted_type_bindings(),
    })
}

pub(super) fn restore_frame(snapshot: LocalsSnapshot) -> Result<Frame, String> {
    Frame::from_snapshot_with_type_bindings(
        snapshot
            .locals
            .into_iter()
            .map(|(symbol, value)| Ok((symbol, value.restore()?)))
            .collect::<Result<Vec<_>, String>>()?,
        snapshot.type_bindings.into_iter().collect(),
    )
}

#[cfg(test)]
mod tests {
    use etas_hir::SymbolId;

    use super::*;
    use crate::value::{ArrayValue, InterpValue};

    #[test]
    fn typed_frame_snapshot_is_detached_from_mutable_runtime_values() {
        let symbol = SymbolId(7);
        let frame = Frame::from_snapshot(vec![(
            symbol,
            InterpValue::Array(ArrayValue::new(vec![InterpValue::i32(1)])),
        )])
        .expect("test frame should build");
        let snapshot = capture_frame(&frame).expect("frame should snapshot");

        let InterpValue::Array(array) = frame.get(symbol).expect("runtime value should exist")
        else {
            panic!("runtime value should remain an array");
        };
        array.borrow_mut().push(InterpValue::i32(2));

        let restored = restore_frame(snapshot).expect("snapshot should restore");
        let InterpValue::Array(array) = restored.get(symbol).expect("snapshot value should exist")
        else {
            panic!("snapshot value should remain an array");
        };
        assert_eq!(array.snapshot(), vec![InterpValue::i32(1)]);
    }
}

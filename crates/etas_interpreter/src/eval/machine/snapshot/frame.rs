use crate::control::{CallTargetChildren, CallTargetLink, ContinuationLink, Frame};
use crate::orchestration::{
    CallTargetSnapshot, CallTargetSnapshotChildren, CallTargetSnapshotLink, ContinuationSnapshot,
    ContinuationSnapshotLink, LocalsSnapshot,
};

#[derive(Default)]
pub(crate) struct RestoreContext {
    frames: std::collections::HashMap<u64, (LocalsSnapshot, Frame)>,
    active: std::collections::HashSet<u64>,
    // Consuming restore retains the source owner with each cached node so its
    // address cannot be recycled while this restore operation is still active.
    // Only completed nodes are cached; no cache survives into another restore.
    pub(super) continuations: std::collections::HashMap<
        std::ptr::NonNull<ContinuationSnapshot>,
        (ContinuationSnapshotLink, ContinuationLink),
    >,
    pub(super) call_nodes: std::collections::HashMap<
        *const CallTargetSnapshot,
        (CallTargetSnapshotLink, CallTargetLink),
    >,
    pub(super) call_tables: std::collections::HashMap<
        *const Vec<CallTargetSnapshot>,
        (CallTargetSnapshotChildren, CallTargetChildren),
    >,
}

impl RestoreContext {
    fn frame(&mut self, snapshot: LocalsSnapshot) -> Result<Frame, String> {
        if snapshot.id == 0 {
            return Err("snapshot frame identity must be nonzero".into());
        }
        if let Some((definition, frame)) = self.frames.get(&snapshot.id) {
            if definition != &snapshot {
                return Err("conflicting definitions for snapshot frame identity".into());
            }
            return Ok(frame.clone());
        }
        if !self.active.insert(snapshot.id) {
            return Err("cyclic local-frame snapshot definitions".into());
        }
        let locals = snapshot
            .locals
            .iter()
            .cloned()
            .map(|(symbol, value)| Ok((symbol, value.restore_with(self)?)))
            .collect::<Result<Vec<_>, String>>()?;
        let frame = Frame::from_snapshot_with_type_bindings(
            locals,
            snapshot.type_bindings.iter().cloned().collect(),
        )?;
        self.active.remove(&snapshot.id);
        self.frames.insert(snapshot.id, (snapshot, frame.clone()));
        Ok(frame)
    }
}

#[cfg(test)]
pub(super) fn capture_frame(frame: &Frame) -> Result<LocalsSnapshot, String> {
    super::capture_context::CaptureContext::default().frame(frame)
}

pub(super) fn restore_frame(
    snapshot: LocalsSnapshot,
    context: &mut RestoreContext,
) -> Result<Frame, String> {
    context.frame(snapshot)
}

#[cfg(test)]
mod tests {
    use etas_hir::SymbolId;

    use super::*;
    use crate::value::{ArrayValue, InterpValue};

    #[test]
    fn installed_scope_slots_are_shared_reused_and_captured_without_identity_conflicts() {
        use crate::{plan::SlotLayoutTable, testing::allocation::measure};
        use std::sync::Arc;
        for count in [1000, 2000, 4000] {
            let mut frame = Frame::new(Arc::new(SlotLayoutTable::from_symbols(vec![SymbolId(0)])));
            frame.insert(SymbolId(0), InterpValue::i32(42));
            let keep = frame.snapshot_symbols();
            let alias = frame.clone();
            let layout = SlotLayoutTable::from_symbols((1..=count).map(SymbolId).collect());
            let (_, cost) = measure(|| frame.install_scope_layout(&layout));
            eprintln!("install {count} handler slots: {cost:?}");
            assert!(cost.count <= 2, "{cost:?}");
            let (_, repeat) = measure(|| frame.install_scope_layout(&layout));
            assert_eq!(repeat.count, 0, "{repeat:?}");
            frame.insert(
                SymbolId(count),
                InterpValue::Array(vec![InterpValue::i32(1)].into()),
            );
            assert_eq!(alias.get(SymbolId(count)), frame.get(SymbolId(count)));
            let saved = capture_frame(&frame).unwrap();
            assert_eq!(saved, capture_frame(&alias).unwrap());
            let mut context = RestoreContext::default();
            let mut restored = restore_frame(saved.clone(), &mut context).unwrap();
            let restored_alias =
                restore_frame(capture_frame(&alias).unwrap(), &mut context).unwrap();
            restored.install_scope_layout(&layout);
            assert!(restored.set(SymbolId(count), InterpValue::i32(99)));
            assert_eq!(
                restored_alias.get(SymbolId(count)),
                Some(InterpValue::i32(99))
            );
            frame.cleanup_to(&keep);
            assert_eq!(alias.get(SymbolId(count)), None);
            assert_eq!(alias.get(SymbolId(0)), Some(InterpValue::i32(42)));
            let old = restore_frame(saved, &mut RestoreContext::default()).unwrap();
            assert_eq!(
                old.get(SymbolId(count)),
                Some(InterpValue::Array(vec![InterpValue::i32(1)].into()))
            );
        }
    }

    #[test]
    fn capturing_frame_shares_strings_and_only_allocates_the_local_table() {
        for count in [1000, 2000, 4000] {
            let frame = Frame::from_snapshot(
                (0..count)
                    .map(|i| (SymbolId(i), InterpValue::String("x".repeat(128).into())))
                    .collect(),
            )
            .unwrap();
            let (snapshot, allocations) =
                crate::testing::allocation::measure(|| capture_frame(&frame).unwrap());
            assert_eq!(
                allocations.count, 2,
                "only local slots and immutable backing header"
            );
            assert_eq!(snapshot.locals.len(), count as usize);
            assert_eq!(snapshot.locals[0].0, SymbolId(0));
        }
    }

    #[test]
    fn captured_frame_definition_clone_and_shared_comparison_do_not_copy_payloads() {
        for count in [1000, 2000, 4000] {
            let frame = Frame::from_snapshot(
                (0..count)
                    .map(|i| (SymbolId(i), InterpValue::Bytes(vec![7; 1024].into())))
                    .collect(),
            )
            .unwrap();
            let snapshot = capture_frame(&frame).unwrap();
            let (alias, cost) = crate::testing::allocation::measure(|| snapshot.clone());
            assert_eq!(cost.count, 0, "n={count}: {cost:?}");
            assert_eq!(cost.bytes, 0);
            assert!(std::rc::Rc::ptr_eq(&alias.locals, &snapshot.locals));
            let (equal, cost) = crate::testing::allocation::measure(|| alias == snapshot);
            assert!(equal);
            assert_eq!(cost.count, 0);
            // Malformed independent definitions must still be compared by value.
            let mut changed = alias.clone();
            std::rc::Rc::make_mut(&mut changed.locals)[0].1 =
                crate::orchestration::ValueSnapshot::Unit;
            assert_ne!(changed, snapshot);
            assert_eq!(alias, snapshot);
        }
    }

    #[test]
    fn cloned_checkpoint_definitions_remain_independent_of_restored_frame_mutations() {
        let symbol = SymbolId(1);
        let frame = Frame::from_snapshot(vec![(
            symbol,
            InterpValue::Array(ArrayValue::new(vec![InterpValue::Record(
                crate::value::RecordValue::new(vec![(
                    "bytes".into(),
                    InterpValue::Bytes(vec![7; 4096].into()),
                )]),
            )])),
        )])
        .unwrap();
        let snapshot = capture_frame(&frame).unwrap();
        let retained = snapshot.clone();
        let mut context = RestoreContext::default();
        let mut restored = restore_frame(snapshot, &mut context).unwrap();
        restored
            .with_local_mut(symbol, |value| {
                let InterpValue::Array(values) = value else {
                    panic!("array")
                };
                let values = values.borrow_mut();
                let InterpValue::Record(fields) = &mut values[0] else {
                    panic!("record")
                };
                *fields.field_mut("bytes").unwrap() = InterpValue::Bytes(vec![9; 4096].into());
            })
            .unwrap();
        assert_eq!(retained, capture_frame(&frame).unwrap());
        let independent = restore_frame(retained, &mut RestoreContext::default()).unwrap();
        assert_eq!(independent.get(symbol), frame.get(symbol));
        assert_ne!(independent.get(symbol), restored.get(symbol));
    }

    #[test]
    fn restoration_preserves_shared_frames_but_not_distinct_equal_frames() {
        let symbol = SymbolId(7);
        let frame = Frame::from_snapshot(vec![(symbol, InterpValue::i32(1))]).unwrap();
        let distinct = Frame::from_snapshot(vec![(symbol, InterpValue::i32(1))]).unwrap();
        let mut context = RestoreContext::default();
        let snapshot = capture_frame(&frame).unwrap();
        let mut first = restore_frame(snapshot.clone(), &mut context).unwrap();
        let shared = restore_frame(snapshot, &mut context).unwrap();
        let distinct = restore_frame(capture_frame(&distinct).unwrap(), &mut context).unwrap();
        assert!(first.set(symbol, InterpValue::i32(2)));
        assert_eq!(shared.get(symbol), Some(InterpValue::i32(2)));
        assert_eq!(distinct.get(symbol), Some(InterpValue::i32(1)));
        assert_ne!(first.snapshot_id(), frame.snapshot_id());
    }

    #[test]
    fn restoration_rejects_inconsistent_frame_identity_definitions() {
        let symbol = SymbolId(7);
        let frame = Frame::from_snapshot(vec![(symbol, InterpValue::i32(1))]).unwrap();
        let snapshot = capture_frame(&frame).unwrap();
        let mut context = RestoreContext::default();
        restore_frame(snapshot.clone(), &mut context).unwrap();
        let mut bad = snapshot;
        std::rc::Rc::make_mut(&mut bad.locals)[0].1 =
            crate::orchestration::ValueSnapshot::capture(&InterpValue::i32(2)).unwrap();
        assert!(
            restore_frame(bad.clone(), &mut context)
                .unwrap_err()
                .contains("conflicting definitions")
        );
        bad.id = 0;
        assert!(
            restore_frame(bad, &mut context)
                .unwrap_err()
                .contains("nonzero")
        );
    }

    #[test]
    fn typed_frame_snapshot_is_detached_from_mutable_runtime_values() {
        let symbol = SymbolId(7);
        let mut frame = Frame::from_snapshot(vec![(
            symbol,
            InterpValue::Array(ArrayValue::new(vec![InterpValue::i32(1)])),
        )])
        .expect("test frame should build");
        let snapshot = capture_frame(&frame).expect("frame should snapshot");

        frame
            .with_local_mut(symbol, |value| {
                let InterpValue::Array(array) = value else {
                    panic!("runtime value should remain an array");
                };
                array.borrow_mut().push(InterpValue::i32(2));
                assert_eq!(array.borrow().len(), 2);
            })
            .expect("existing runtime slot");

        let restored = restore_frame(snapshot, &mut RestoreContext::default())
            .expect("snapshot should restore");
        let InterpValue::Array(array) = restored.get(symbol).expect("snapshot value should exist")
        else {
            panic!("snapshot value should remain an array");
        };
        assert_eq!(array.snapshot(), vec![InterpValue::i32(1)]);
    }
}

use super::*;
use crate::{
    control::Frame,
    plan::SlotLayoutTable,
    testing::allocation::measure,
    value::{ArrayValue, ListValue, MapValue, RecordValue},
};
use etas_hir::SymbolId;
use etas_types::TypeId;
use std::sync::Arc;

#[test]
fn wide_adt_frame_reads_share_field_storage_without_allocating() {
    for count in [1000, 2000, 4000] {
        let fields = SharedFields::new(
            (0..count)
                .map(|_| InterpValue::String("payload".repeat(128).into()))
                .collect(),
        );
        let pointer = fields.as_ptr();
        for value in [
            InterpValue::Tuple(fields.clone()),
            InterpValue::Variant {
                name: "Wide".into(),
                fields: fields.clone(),
            },
        ] {
            let mut frame = Frame::new(Arc::new(SlotLayoutTable::from_symbols(vec![SymbolId(0)])));
            frame.insert(SymbolId(0), value);
            let (alias, cost) = measure(|| frame.get(SymbolId(0)).unwrap());
            assert_eq!(cost.count, 0, "n={count}: {cost:?}");
            assert_eq!(cost.bytes, 0);
            let (InterpValue::Tuple(alias) | InterpValue::Variant { fields: alias, .. }) = alias
            else {
                panic!("aggregate");
            };
            assert_eq!(alias.as_ptr(), pointer);
        }
    }
}

#[test]
fn consuming_shared_fields_copies_slots_only_when_an_alias_is_live() {
    for count in [1000, 2000, 4000] {
        let fields = SharedFields::new(
            (0..count)
                .map(|_| InterpValue::Bytes(vec![7; 4096].into()))
                .collect(),
        );
        let original = fields.as_ptr();
        let (copied, cost) = measure(|| fields.clone().into_values());
        assert_eq!(cost.count, 1);
        assert_eq!(cost.bytes, count * std::mem::size_of::<InterpValue>());
        assert_ne!(copied.as_ptr(), original);
        for (copy, source) in copied.iter().zip(fields.iter()) {
            let (InterpValue::Bytes(copy), InterpValue::Bytes(source)) = (copy, source) else {
                panic!("bytes");
            };
            assert_eq!(copy.as_ptr(), source.as_ptr());
        }
        let (moved, cost) = measure(|| fields.into_values());
        assert_eq!(cost.count, 0);
        assert_eq!(moved.as_ptr(), original);
    }
}

#[test]
fn consuming_single_payload_never_copies_a_field_vector_or_nested_adt() {
    for count in [1000, 2000, 4000] {
        let mut nested = InterpValue::Bytes(vec![42; count].into());
        for _ in 0..count {
            nested = InterpValue::OptionSome(nested.into());
        }
        let inner = SharedValue::new(nested);
        let (alias, cost) = measure(|| inner.clone().into_value());
        assert_eq!(cost.count, 0);
        let InterpValue::OptionSome(alias) = alias else {
            panic!("Some")
        };
        let InterpValue::OptionSome(source) = inner.as_ref() else {
            panic!("Some")
        };
        assert!(std::ptr::eq(alias.as_ref(), source.as_ref()));

        let fields = SharedFields::new(vec![InterpValue::OptionSome(alias)]);
        let (copied, cost) = measure(|| fields.clone().into_single().unwrap());
        assert_eq!(cost.count, 0);
        let (moved, cost) = measure(|| fields.into_single().unwrap());
        assert_eq!(cost.count, 0);
        let (InterpValue::OptionSome(copied), InterpValue::OptionSome(moved)) = (copied, moved)
        else {
            panic!("Some");
        };
        assert!(std::ptr::eq(copied.as_ref(), moved.as_ref()));
    }
    assert!(SharedFields::default().into_single().is_none());
    assert!(
        SharedFields::new(vec![InterpValue::Unit, InterpValue::Unit])
            .into_single()
            .is_none()
    );
}

#[test]
fn nominal_payload_mutation_detaches_only_the_changed_owned_path() {
    let retained = SharedValue::new(InterpValue::Nominal {
        ty: TypeId(41),
        value: InterpValue::Record(RecordValue::new(vec![(
            "text".into(),
            InterpValue::String("old".into()),
        )]))
        .into(),
    });
    let mut changed = retained.clone();
    let InterpValue::Nominal { ty, value } = changed.make_mut() else {
        panic!("nominal")
    };
    assert_eq!(*ty, TypeId(41));
    let InterpValue::Record(fields) = value.make_mut() else {
        panic!("record")
    };
    *fields.field_mut("text").unwrap() = InterpValue::String("new".into());
    let InterpValue::Nominal { ty, value } = retained.as_ref() else {
        panic!("nominal")
    };
    assert_eq!(*ty, TypeId(41));
    let InterpValue::Record(fields) = value.as_ref() else {
        panic!("record")
    };
    assert_eq!(fields.get("text"), Some(InterpValue::String("old".into())));
    drop(changed);
    assert_eq!(fields.get("text"), Some(InterpValue::String("old".into())));
}

#[test]
fn deep_adt_release_is_iterative_through_collection_and_record_children() {
    // Run on the ordinary test thread stack, with no stack-size override.
    // A retained child checks that releasing an ancestor does not drain aliases.
    let retained = SharedValue::new(InterpValue::String("retained".into()));
    let mut value = InterpValue::OptionSome(retained.clone());
    for depth in 0..30_000 {
        value = match depth % 7 {
            0 => InterpValue::Nominal {
                ty: TypeId(1),
                value: value.into(),
            },
            1 => InterpValue::Variant {
                name: "Next".into(),
                fields: vec![value].into(),
            },
            2 => InterpValue::Tuple(vec![value].into()),
            3 => InterpValue::Array(ArrayValue::new(vec![value])),
            4 => InterpValue::List(ListValue::new(vec![value])),
            5 => InterpValue::Record(RecordValue::new(vec![("next".into(), value)])),
            _ => InterpValue::Map(MapValue::new(vec![(InterpValue::Unit, value)])),
        };
    }
    let root = SharedValue::new(value);
    let alias = root.clone();
    let (_, cost) = measure(|| drop(root));
    assert_eq!(cost.count, 0, "a shared root does not walk its subtree");
    drop(alias);
    assert_eq!(retained.as_ref(), &InterpValue::String("retained".into()));
}

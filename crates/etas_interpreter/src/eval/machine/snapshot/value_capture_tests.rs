use super::*;
use crate::{
    testing::allocation::measure,
    value::{ArrayValue, DequeValue, ListValue, MapValue, SetValue, SliceValue},
};

#[test]
fn container_capture_allocates_only_the_owned_snapshot_slots() {
    for count in [1000, 2000, 4000] {
        let values = || (0..count).map(InterpValue::i32).collect::<Vec<_>>();
        let pairs = || {
            (0..count)
                .map(|i| (InterpValue::i32(i), InterpValue::i32(i)))
                .collect::<Vec<_>>()
        };
        for runtime in [
            InterpValue::Tuple(values().into()),
            InterpValue::Array(ArrayValue::new(values())),
            InterpValue::List(ListValue::new(values())),
            InterpValue::Slice(SliceValue::new(values())),
            InterpValue::Set(SetValue::new(values())),
            InterpValue::Deque(DequeValue::new(values())),
            InterpValue::Queue(DequeValue::new(values())),
            InterpValue::Stack(ArrayValue::new(values())),
            InterpValue::OrderedSet(SetValue::new(values())),
        ] {
            let (snapshot, cost) = measure(|| ValueSnapshot::capture(&runtime).unwrap());
            assert_eq!(
                cost.count,
                1,
                "{}, n={count}: {cost:?}",
                runtime.kind_name()
            );
            assert_eq!(
                cost.bytes,
                count as usize * std::mem::size_of::<ValueSnapshot>()
            );
            assert_eq!(snapshot.restore().unwrap(), runtime);
        }
        for runtime in [
            InterpValue::Map(MapValue::new(pairs())),
            InterpValue::OrderedMap(MapValue::new(pairs())),
            InterpValue::PriorityQueue(MapValue::new(pairs())),
        ] {
            let (snapshot, cost) = measure(|| ValueSnapshot::capture(&runtime).unwrap());
            assert_eq!(
                cost.count,
                1,
                "{}, n={count}: {cost:?}",
                runtime.kind_name()
            );
            assert_eq!(
                cost.bytes,
                count as usize * std::mem::size_of::<(ValueSnapshot, ValueSnapshot)>()
            );
            assert_eq!(snapshot.restore().unwrap(), runtime);
        }
    }
}

#[test]
fn snapshot_capture_shares_immutable_payloads_between_tree_occurrences() {
    // Sharing is in-memory only. The wire format remains a value tree.
    for count in [1000, 2000, 4000] {
        let bytes = InterpValue::Bytes(vec![7; count].into());
        let text = InterpValue::String("x".repeat(count).into());
        let runtime = InterpValue::Array(ArrayValue::new(vec![
            bytes.clone(),
            bytes,
            text.clone(),
            text,
        ]));
        let (snapshot, cost) = measure(|| ValueSnapshot::capture(&runtime).unwrap());
        assert_eq!(
            cost.count, 1,
            "only the snapshot vector, no payload buffers"
        );
        assert_eq!(cost.bytes, 4 * std::mem::size_of::<ValueSnapshot>());
        let ValueSnapshot::Array(fields) = &snapshot else {
            panic!("array")
        };
        let (ValueSnapshot::Bytes(a), ValueSnapshot::Bytes(b)) = (&fields[0], &fields[1]) else {
            panic!("bytes")
        };
        assert_eq!(a, b);
        assert_eq!(a.as_ptr(), b.as_ptr());
        let (ValueSnapshot::String(a), ValueSnapshot::String(b)) = (&fields[2], &fields[3]) else {
            panic!("strings")
        };
        assert_eq!(a.as_ptr(), b.as_ptr());
        assert_eq!(snapshot.restore().unwrap(), runtime);
    }
}

#[test]
fn immutable_payload_capture_clone_restore_allocate_nothing_and_isolate_mutation() {
    for count in [1000, 2000, 4000] {
        for runtime in [
            InterpValue::String("x".repeat(count).into()),
            InterpValue::Bytes(vec![7; count].into()),
        ] {
            let ((snapshot, restored), cost) = measure(|| {
                let snapshot = ValueSnapshot::capture(&runtime).unwrap();
                let restored = snapshot.clone().restore().unwrap();
                (snapshot, restored)
            });
            assert_eq!(cost.count, 0, "n={count}: {cost:?}");
            assert_eq!(cost.bytes, 0);
            match (runtime.clone(), restored) {
                (InterpValue::String(mut original), InterpValue::String(mut restored)) => {
                    assert_eq!(original.as_ptr(), restored.as_ptr());
                    original.push_str(" original");
                    restored.push_str(" restored");
                    assert_eq!(original.as_str(), format!("{} original", "x".repeat(count)));
                    assert_eq!(restored.as_str(), format!("{} restored", "x".repeat(count)));
                }
                (InterpValue::Bytes(original), InterpValue::Bytes(restored)) => {
                    assert_eq!(original.as_ptr(), restored.as_ptr());
                    let mut original = original.into_vec();
                    let mut restored = restored.into_vec();
                    original[0] = 8;
                    restored[0] = 9;
                    assert_eq!(original[0], 8);
                    assert_eq!(restored[0], 9);
                }
                _ => panic!("preserved payload kind"),
            }
            assert_eq!(snapshot.restore().unwrap(), runtime);
        }
    }
}

#[test]
fn bounded_recursive_snapshot_capture_clone_compare_drop_preserve_payload_identity() {
    for depth in [32, 64, 128] {
        let mut runtime = InterpValue::Bytes(vec![7; 4096].into());
        for _ in 0..depth {
            runtime = InterpValue::OptionSome(runtime.into());
        }
        let snapshot = ValueSnapshot::capture(&runtime).unwrap();
        let copy = snapshot.clone();
        assert_eq!(copy, snapshot);
        assert_eq!(copy.restore().unwrap(), runtime);
        drop(snapshot);
        drop(runtime);
    }
}

#[test]
fn deep_variant_snapshot_capture_clone_compare_restore_and_drop() {
    for depth in [1000, 2000, 4000, 30000] {
        let mut runtime = InterpValue::Variant {
            name: "End".into(),
            fields: vec![].into(),
        };
        for _ in 0..depth {
            runtime = InterpValue::Variant {
                name: "Next".into(),
                fields: vec![runtime].into(),
            };
        }
        let snapshot = ValueSnapshot::capture(&runtime).unwrap();
        let copy = snapshot.clone();
        assert!(
            copy == snapshot,
            "deep snapshot clone must preserve all nodes"
        );
        let restored = copy.restore().unwrap();
        let mut current = &restored;
        for _ in 0..depth {
            let InterpValue::Variant { name, fields } = current else {
                panic!("variant")
            };
            assert_eq!(name, "Next");
            assert_eq!(fields.len(), 1);
            current = &fields[0];
        }
        let InterpValue::Variant { name, fields } = current else {
            panic!("terminal variant")
        };
        assert_eq!(name, "End");
        assert!(fields.is_empty());
        drop(snapshot);
        drop(restored);
        drop(runtime);
    }
}

#[test]
fn deep_mixed_snapshot_preserves_identity_and_cleans_up_capture_failure() {
    use crate::value::{HostHandleValue, RecordValue};
    use etas_types::TypeId;
    let mut runtime = InterpValue::String("leaf".into());
    for depth in 0..30000 {
        runtime = match depth % 6 {
            0 => InterpValue::OptionSome(runtime.into()),
            1 => InterpValue::Nominal {
                ty: TypeId(7),
                value: runtime.into(),
            },
            2 => InterpValue::Array(ArrayValue::new(vec![runtime])),
            3 => InterpValue::List(ListValue::new(vec![runtime])),
            4 => InterpValue::Record(RecordValue::new(vec![("child".into(), runtime)])),
            _ => InterpValue::Map(MapValue::new(vec![(InterpValue::Unit, runtime)])),
        };
    }
    let snapshot = ValueSnapshot::capture(&runtime).unwrap();
    let mut different = snapshot.clone();
    let mut current = &mut different;
    loop {
        current = match current {
            ValueSnapshot::OptionSome(child) => child,
            ValueSnapshot::Nominal { ty, .. } => {
                *ty = TypeId(8);
                break;
            }
            ValueSnapshot::Array(values) | ValueSnapshot::List(values) => &mut values[0],
            ValueSnapshot::Record(values) => &mut values[0].1,
            ValueSnapshot::Map(values) => &mut values[0].1,
            _ => panic!("mixed snapshot lost its expected shape"),
        };
    }
    assert!(
        snapshot != different,
        "nominal identity must participate in equality"
    );
    let restored = snapshot.clone().restore().unwrap();
    assert!(snapshot == ValueSnapshot::capture(&restored).unwrap());

    let with_live_handle = InterpValue::Tuple(
        vec![
            runtime,
            InterpValue::HostHandle(HostHandleValue::browser_session(
                TypeId(9),
                "not-serializable".into(),
            )),
        ]
        .into(),
    );
    let error = ValueSnapshot::capture(&with_live_handle).unwrap_err();
    assert!(
        error.contains("live browser_session host handles"),
        "{error}"
    );

    let malformed = ValueSnapshot::Tuple(
        vec![
            snapshot,
            ValueSnapshot::WorkspacePath {
                region: String::new(),
                relative: String::new(),
            },
        ]
        .into(),
    );
    assert!(
        malformed.restore().is_err(),
        "restore must retain validation after a deep child"
    );
}

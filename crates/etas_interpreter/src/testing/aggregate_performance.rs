use crate::orchestration::ValueSnapshot;
use crate::value::{
    ArrayValue, InterpValue, ListValue, MapValue, RecordValue, SetValue, SliceValue,
};

use super::allocation::measure;

fn strings(count: usize) -> Vec<InterpValue> {
    (0..count)
        .map(|i| InterpValue::String(format!("{i:08}{}", "x".repeat(128)).into()))
        .collect()
}

#[test]
fn host_json_text_encoding_allocates_output_not_an_intermediate_payload_graph() {
    use etas_host::{HostValue, host_value_to_json, host_value_to_json_string};
    for count in [1000, 2000, 4000] {
        let value = HostValue::Record(vec![(
            "nested".into(),
            HostValue::List(
                (0..count)
                    .map(|_| HostValue::String("payload".repeat(128)))
                    .collect(),
            ),
        )]);
        let (reference, old) = measure(|| host_value_to_json(&value).unwrap().to_string());
        let (serialized, current) = measure(|| host_value_to_json_string(&value).unwrap());
        assert_eq!(serialized, reference);
        assert!(old.count >= count, "reference copies every string");
        assert!(
            current.count < 32,
            "only output growth and the object field index: {current:?}"
        );
        assert!(current.bytes < old.bytes);
        eprintln!(
            "host JSON n={count}, serialized={} bytes, old={old:?}, streaming={current:?}",
            serialized.len()
        );
    }
}

#[test]
fn mutable_value_operations_enforce_copy_on_write_without_caller_guards() {
    let mut array = ArrayValue::new(strings(3));
    let alias = array.clone();
    array.borrow_mut()[0] = InterpValue::Unit;
    assert!(matches!(alias.borrow()[0], InterpValue::String(_)));

    let mut list = ListValue::new(strings(3));
    let alias = list.clone();
    *list.get_mut(0).unwrap() = InterpValue::Unit;
    assert!(matches!(alias.get(0), Some(InterpValue::String(_))));

    let mut map = MapValue::new(vec![(InterpValue::i32(1), InterpValue::i32(2))]);
    let alias = map.clone();
    map.borrow_mut()[0].1 = InterpValue::i32(3);
    assert_eq!(alias.borrow()[0].1, InterpValue::i32(2));

    let mut set = SetValue::new(vec![InterpValue::i32(1)]);
    let alias = set.clone();
    set.borrow_mut().clear();
    assert_eq!(alias.borrow().len(), 1);

    let mut record = RecordValue::new(vec![("field".into(), InterpValue::i32(1))]);
    let alias = record.clone();
    *record.field_mut("field").unwrap() = InterpValue::i32(2);
    assert_eq!(alias.get("field"), Some(InterpValue::i32(1)));
    let alias = record.clone();
    record.borrow_mut()[0].0 = "renamed".into();
    assert_eq!(alias.get("field"), Some(InterpValue::i32(2)));
    assert_eq!(record.get("field"), None);
    assert_eq!(record.get("renamed"), Some(InterpValue::i32(2)));
}

fn aggregates(count: usize) -> Vec<InterpValue> {
    vec![
        InterpValue::Array(ArrayValue::new(strings(count))),
        InterpValue::List(ListValue::new(strings(count))),
        InterpValue::Slice(SliceValue::new(strings(count))),
        InterpValue::Set(SetValue::new(strings(count))),
        InterpValue::Map(MapValue::new(
            strings(count)
                .into_iter()
                .map(|v| (v, InterpValue::Unit))
                .collect(),
        )),
        InterpValue::Record(RecordValue::new(
            strings(count)
                .into_iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v))
                .collect(),
        )),
    ]
}

#[test]
fn closure_capture_cost_depends_on_free_variables_not_enclosing_locals() {
    use crate::{control::Frame, plan::SlotLayoutTable};
    use etas_hir::SymbolId;
    use std::sync::Arc;
    let captured_symbol = SymbolId(0);
    let layout = Arc::new(SlotLayoutTable::from_symbols(vec![captured_symbol]));
    let mut baseline = None;
    for count in [1000, 2000, 4000] {
        let frame = Frame::from_snapshot(
            strings(count)
                .into_iter()
                .enumerate()
                .map(|(i, value)| (SymbolId(i as u32), value))
                .collect(),
        )
        .unwrap();
        let (captured, allocations) =
            measure(|| frame.capture(layout.clone(), &[captured_symbol]).unwrap());
        assert_eq!(captured.sorted_locals().len(), 1);
        assert_eq!(
            allocations.count, 2,
            "one slot buffer and one Rc; captured text shares backing"
        );
        if let Some(bytes) = baseline {
            assert_eq!(allocations.bytes, bytes);
        }
        baseline = Some(allocations.bytes);
        assert!(
            frame
                .capture(layout.clone(), &[SymbolId(count as u32)])
                .is_err()
        );
    }
}

#[test]
fn slice_views_share_backing_without_copying_and_materialize_only_the_window() {
    for count in [1000, 2000, 4000] {
        let mut values = ArrayValue::new(strings(count));
        let pointer = values.borrow()[3..].as_ptr();
        let (slice, allocations) = measure(|| {
            SliceValue::from_array(values.clone(), 1..count)
                .unwrap()
                .slice(2..5)
                .unwrap()
        });
        assert_eq!(allocations.count, 0);
        assert_eq!(slice.borrow().as_ptr(), pointer);
        assert_eq!(slice.borrow().len(), 3);
        values.make_unique();
        values.borrow_mut()[3] = InterpValue::Unit;
        assert!(matches!(slice.borrow()[0], InterpValue::String(_)));

        let snapshot = ValueSnapshot::capture(&InterpValue::Slice(slice.clone())).unwrap();
        let restored = snapshot.restore().unwrap();
        assert_eq!(restored, InterpValue::Slice(slice.clone()));
        let (materialized, allocations) = measure(|| slice.into_values());
        assert_eq!(materialized.len(), 3);
        assert_eq!(allocations.count, 0, "unique window must reuse backing");
    }
}

#[test]
fn shared_slice_materialization_copies_only_selected_elements() {
    let values = ArrayValue::new(strings(1000));
    let slice = SliceValue::from_array(values.clone(), 2..5).unwrap();
    let (materialized, allocations) = measure(|| slice.into_values());
    assert_eq!(materialized.len(), 3);
    assert_eq!(
        allocations.count, 1,
        "one vector; selected text remains shared"
    );
    assert_eq!(values.borrow().len(), 1000);
    assert!(SliceValue::from_array(values.clone(), 0..1001).is_none());
    assert!(
        SliceValue::from_array(values.clone(), 10..10)
            .unwrap()
            .borrow()
            .is_empty()
    );
    assert!(
        SliceValue::from_array(values, 2..5)
            .unwrap()
            .slice(0..4)
            .is_none()
    );
}

#[test]
fn aggregate_equality_does_not_materialize_payloads() {
    for count in [1000, 2000, 4000] {
        for (left, right) in aggregates(count).into_iter().zip(aggregates(count)) {
            let (equal, allocations) = measure(|| left == right);
            assert!(equal);
            assert_eq!(allocations.count, 0, "{count} elements: {allocations:?}");
            let (equal, allocations) = measure(|| left == left.clone());
            assert!(equal);
            assert_eq!(allocations.count, 0, "shared backing: {allocations:?}");
        }
    }
}

#[test]
fn checkpoint_capture_avoids_intermediate_payload_copies() {
    for count in [1000, 2000, 4000] {
        for value in aggregates(count) {
            let field_names = if matches!(value, InterpValue::Record(_)) {
                count
            } else {
                0
            };
            let (snapshot, allocations) = measure(|| ValueSnapshot::capture(&value).unwrap());
            eprintln!("capture n={count} field_names={field_names}: {allocations:?}");
            assert_eq!(
                allocations.count,
                field_names + 1,
                "only snapshot slots and owned field names, n={count}: {allocations:?}"
            );
            assert_eq!(snapshot.restore().unwrap(), value);
        }
    }
}

#[test]
fn aggregate_extraction_reuses_unique_backing_and_preserves_aliases() {
    macro_rules! check {
        ($container:ident, $elements:expr) => {{
            let original = $elements;
            let pointer = original.as_ptr();
            let container = $container::new(original);
            let (values, allocations) = measure(|| container.into_values());
            assert_eq!(values.as_ptr(), pointer);
            assert_eq!(allocations.count, 0);
            let container = $container::new(values);
            let alias = container.clone();
            let mut extracted = container.into_values();
            assert_ne!(extracted.as_ptr(), alias.borrow().as_ptr());
            extracted.clear();
            assert!(!alias.borrow().is_empty());
        }};
    }
    for count in [1000, 2000, 4000] {
        check!(ArrayValue, strings(count));
        let original = strings(count);
        let pointers: Vec<_> = original
            .iter()
            .map(|value| match value {
                InterpValue::String(value) => value.as_ptr(),
                _ => unreachable!(),
            })
            .collect();
        let list = ListValue::new(original);
        let (values, allocations) = measure(|| list.into_values());
        assert_eq!(allocations.count, 1, "list output vector only");
        for (value, pointer) in values.iter().zip(pointers) {
            let InterpValue::String(value) = value else {
                panic!("string")
            };
            assert_eq!(value.as_ptr(), pointer);
        }
        let list = ListValue::new(values);
        let alias = list.clone();
        let mut values = list.into_values();
        values.clear();
        assert_eq!(alias.len(), count);
        check!(SliceValue, strings(count));
        check!(SetValue, strings(count));
        check!(
            MapValue,
            strings(count)
                .into_iter()
                .map(|v| (v, InterpValue::Unit))
                .collect::<Vec<_>>()
        );
        check!(
            RecordValue,
            strings(count)
                .into_iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn borrowed_capture_preserves_nested_snapshot_independence() {
    let mut inner = ArrayValue::new(vec![InterpValue::Bytes(vec![7; 4096].into())]);
    let mut outer = ArrayValue::new(vec![InterpValue::Array(inner.clone())]);
    let snapshot = ValueSnapshot::capture(&InterpValue::Array(outer.clone())).unwrap();
    inner.borrow_mut()[0] = InterpValue::Bytes(vec![9; 4096].into());
    outer.borrow_mut().clear();
    assert_eq!(
        snapshot.restore().unwrap(),
        InterpValue::Array(ArrayValue::new(vec![InterpValue::Array(ArrayValue::new(
            vec![InterpValue::Bytes(vec![7; 4096].into())]
        ))]))
    );
}

#[test]
fn aggregate_queries_copy_only_the_selected_payload() {
    for count in [1000, 2000, 4000] {
        let map = MapValue::new(
            strings(count)
                .into_iter()
                .enumerate()
                .map(|(i, v)| (InterpValue::usize(i), v))
                .collect(),
        );
        let key = InterpValue::usize(count - 1);
        let (present, allocations) = measure(|| map.contains_key(&key));
        assert!(present);
        assert_eq!(allocations.count, 0);
        let (value, allocations) = measure(|| map.get(&key));
        assert!(matches!(value, Some(InterpValue::String(_))));
        assert_eq!(allocations.count, 0);
        let missing = InterpValue::usize(count);
        let (value, allocations) = measure(|| map.get(&missing));
        assert!(value.is_none());
        assert_eq!(allocations.count, 0);

        let fields = RecordValue::new(
            strings(count)
                .into_iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v))
                .collect(),
        );
        let name = (count - 1).to_string();
        let (value, allocations) = measure(|| fields.get(&name));
        assert!(matches!(value, Some(InterpValue::String(_))));
        assert_eq!(
            allocations.count, 2,
            "cold dynamic record: index buffer and Arc; shared text"
        );
        let (value, allocations) = measure(|| fields.get(&name));
        assert!(matches!(value, Some(InterpValue::String(_))));
        assert_eq!(allocations.count, 0);

        let values = SetValue::new(strings(count));
        let key = values.borrow().last().unwrap().clone();
        let (present, allocations) = measure(|| values.contains(&key));
        assert!(present);
        assert_eq!(allocations.count, 0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn owned_collection_updates_preserve_aliases_across_argument_calls() {
    use super::{FakeHost, checked_project};
    use crate::{
        Interpreter,
        api::{EntryPoint, RunOptions},
        host::HostServiceAvailability,
    };
    let checked = checked_project(
        r#"
module app.main;
flow next_value() -> string { return "new"; }
flow main() -> bool {
    let original = ["old"];
    let alias = original;
    let updated = original.push(next_value());
    let (rest, popped) = updated.pop();
    let chained = ["first"].push(next_value()).push("last");
    return original == ["old"] && alias == ["old"]
        && updated == ["old", "new"] && rest == ["old"]
        && popped == Some("new") && chained == ["first", "new", "last"];
}
"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

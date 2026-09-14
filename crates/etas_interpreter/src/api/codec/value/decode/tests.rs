use super::*;
use crate::{orchestration::ValueSnapshot, testing::allocation::measure};

// These tests start at the Value API boundary. Avoid serde Value's recursive
// destructor so it cannot mask failures in the interpreter decoder itself.
struct JsonTree(Value);

impl Drop for JsonTree {
    fn drop(&mut self) {
        let mut pending = vec![std::mem::take(&mut self.0)];
        while let Some(value) = pending.pop() {
            match value {
                Value::Array(values) => pending.extend(values),
                Value::Object(fields) => pending.extend(fields.into_values()),
                _ => {}
            }
        }
    }
}

fn deep_wire(depth: usize, mixed: bool) -> JsonTree {
    let mut value = json!({"kind":"number", "type":"i32", "value":"7"});
    for index in 0..depth {
        let kind = if mixed { index % 5 } else { 0 };
        let mut parent = match kind {
            0 => json!({"kind":"variant", "name":"Next", "fields":[]}),
            1 => json!({"kind":"nominal", "ty":13, "value":null}),
            2 => json!({"kind":"option_some", "value":null}),
            3 => json!({"kind":"record", "fields":[{"name":"child", "value":null}]}),
            _ => json!({"kind":"array", "values":[]}),
        };
        match kind {
            0 => parent["fields"].as_array_mut().unwrap().push(value),
            1 | 2 => parent["value"] = value,
            3 => parent["fields"][0]["value"] = value,
            _ => parent["values"].as_array_mut().unwrap().push(value),
        }
        value = parent;
    }
    JsonTree(value)
}

#[test]
fn deep_adt_value_decode_is_stack_safe_and_preserves_identity() {
    for (depth, mixed) in [(1000, false), (1000, true), (4000, true), (30_000, true)] {
        let wire = deep_wire(depth, mixed);
        let decoded = value_from_json(&wire.0).unwrap();
        let snapshot = ValueSnapshot::capture(&decoded).unwrap();
        let mut value = &snapshot;
        for index in (0..depth).rev() {
            value = match (if mixed { index % 5 } else { 0 }, value) {
                (0, ValueSnapshot::Variant { name, fields }) => {
                    assert_eq!(name, "Next");
                    assert_eq!(fields.len(), 1);
                    &fields[0]
                }
                (1, ValueSnapshot::Nominal { ty, value }) => {
                    assert_eq!(*ty, TypeId(13));
                    value
                }
                (2, ValueSnapshot::OptionSome(value)) => value,
                (3, ValueSnapshot::Record(fields)) => {
                    assert_eq!(fields.len(), 1);
                    assert_eq!(fields[0].0, "child");
                    &fields[0].1
                }
                (4, ValueSnapshot::Array(values)) => {
                    assert_eq!(values.len(), 1);
                    &values[0]
                }
                _ => panic!("decoder changed an ADT constructor"),
            };
        }
        assert_eq!(
            value,
            &ValueSnapshot::capture(&InterpValue::i32(7)).unwrap()
        );
    }
}

#[test]
fn aggregate_decode_rejects_invalid_children_and_releases_completed_values() {
    for invalid in [
        json!({"kind":"host_handle", "handle_kind":"stream"}),
        json!({"kind":"number", "type":"u8", "value":"256"}),
        json!({"kind":"nominal", "value":{"kind":"unit"}}),
        json!({"kind":"record", "fields":[{"name":"missing"}]}),
        json!({"kind":"map", "entries":[{"key":{"kind":"unit"}}]}),
        json!({"kind":"trust", "wrapper":"unknown", "value":{"kind":"unit"}}),
        json!({"kind":"range", "start":{"kind":"unit"}, "end":{"kind":"unit"}, "bounds":"unknown"}),
        json!({"kind":"variant", "fields":[]}),
    ] {
        let mut deep = deep_wire(4000, true);
        let mut wire = JsonTree(json!({"kind":"array", "values":[]}));
        wire.0["values"]
            .as_array_mut()
            .unwrap()
            .push(std::mem::take(&mut deep.0));
        wire.0["values"].as_array_mut().unwrap().push(invalid);
        assert!(value_from_json(&wire.0).is_err());
    }
}

#[test]
fn flat_aggregate_decode_allocates_one_output_buffer() {
    let mut previous = None;
    for count in [1000, 2000, 4000] {
        let wire = json!({"kind":"array", "values":(0..count)
            .map(|_| json!({"kind":"bool", "value":true}))
            .collect::<Vec<_>>()});
        let (decoded, allocations) = measure(|| value_from_json(&wire).unwrap());
        assert!(matches!(&decoded, InterpValue::Array(values) if values.borrow().len() == count));
        let payload = count * std::mem::size_of::<InterpValue>();
        assert!(allocations.bytes >= payload);
        assert!(
            allocations.bytes < payload + 4096,
            "extra intermediate value buffer: {allocations:?}"
        );
        if let Some(bytes) = previous {
            assert!(allocations.bytes <= bytes * 2 + 4096);
        }
        previous = Some(allocations.bytes);
    }
}

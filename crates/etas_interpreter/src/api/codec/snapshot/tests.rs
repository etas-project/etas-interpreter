use super::*;
use crate::{
    api::codec,
    control::{CallTarget, Frame},
    testing::allocation::measure,
    value::*,
};
use etas_hir::{HirExprId, SymbolId};
use etas_types::{TrustWrapper, TypeId};
use std::rc::Rc;

#[test]
fn captured_value_json_preserves_the_runtime_wire_contract() {
    let leaf = InterpValue::i32(7);
    let values = vec![leaf.clone(), InterpValue::String("payload".into())];
    let pairs = vec![(InterpValue::String("key".into()), leaf.clone())];
    let cases = vec![
        InterpValue::Unit,
        InterpValue::Bool(true),
        leaf.clone(),
        InterpValue::Bytes(vec![0, 255].into()),
        InterpValue::Nominal {
            ty: TypeId(13),
            value: leaf.clone().into(),
        },
        InterpValue::Trust {
            wrapper: TrustWrapper::Untrusted,
            value: leaf.clone().into(),
        },
        InterpValue::OptionNone,
        InterpValue::OptionSome(leaf.clone().into()),
        InterpValue::Tuple(values.clone().into()),
        InterpValue::Array(ArrayValue::new(values.clone())),
        InterpValue::List(ListValue::new(values.clone())),
        InterpValue::Slice(SliceValue::new(values.clone())),
        InterpValue::Set(SetValue::new(values.clone())),
        InterpValue::OrderedSet(SetValue::new(values.clone())),
        InterpValue::Deque(values.clone().into()),
        InterpValue::Queue(values.clone().into()),
        InterpValue::Stack(ArrayValue::new(values)),
        InterpValue::Map(MapValue::new(pairs.clone())),
        InterpValue::OrderedMap(MapValue::new(pairs.clone())),
        InterpValue::PriorityQueue(MapValue::new(pairs)),
        InterpValue::Record(RecordValue::new(vec![("value".into(), leaf.clone())])),
        InterpValue::Variant {
            name: "Response.Success".into(),
            fields: vec![leaf.clone()].into(),
        },
        InterpValue::Range(RangeValue {
            start: Box::new(leaf),
            end: Box::new(InterpValue::i32(9)),
            bounds: RangeBounds::ClosedOpen,
        }),
    ];
    for value in cases {
        let snapshot = ValueSnapshot::capture(&value).unwrap();
        let json = encode(Node::Value(&snapshot));
        assert_eq!(json, codec::value_json(&value));
        assert_eq!(codec::value_from_json(&json).unwrap(), value);
    }
}

#[test]
fn captured_frame_encoding_removes_runtime_graph_materialization() {
    let mut previous = None;
    for count in [1000, 2000, 4000] {
        let captured = LocalsSnapshot {
            id: 77,
            locals: Rc::new(vec![(
                SymbolId(1),
                ValueSnapshot::Array(
                    (0..count)
                        .map(|_| ValueSnapshot::String("p".repeat(1024).into()))
                        .collect(),
                ),
            )]),
            type_bindings: vec![],
        };
        let target = CallTargetSnapshot::Lambda {
            expr: HirExprId(3),
            captured: captured.clone(),
        };
        let (actual, current) = measure(|| encode(Node::CallTarget(&target)));
        // Reference the former two-step boundary: reconstruct owned runtime
        // locals, then encode the runtime callable. It is never a fallback.
        let (expected, previous_path) = measure(|| {
            let values = captured
                .locals
                .iter()
                .map(|(symbol, value)| (*symbol, value.clone().restore().unwrap()))
                .collect();
            let mut frame = Frame::from_snapshot(values).unwrap();
            frame.set_snapshot_id(captured.id).unwrap();
            codec::machine::call_target_snapshot(&CallTarget::Lambda {
                expr: HirExprId(3),
                captured: frame,
            })
        });
        assert_eq!(actual, expected);
        assert!(
            previous_path.bytes >= current.bytes + count * 1024,
            "encoding must remove at least the intermediate payload copy: {current:?} vs {previous_path:?}"
        );
        if let Some(bytes) = previous {
            assert!(
                current.bytes <= bytes * 2 + 4096,
                "encoding should scale linearly: {current:?}"
            );
        }
        previous = Some(current.bytes);
        eprintln!(
            "captured-frame n={count}: direct={current:?}; restored-runtime={previous_path:?}"
        );
    }
}

#[test]
fn deep_value_encoding_does_not_reserialize_descendants() {
    let mut value = ValueSnapshot::OptionNone;
    for _ in 0..1000 {
        value = ValueSnapshot::Variant {
            name: "Next".into(),
            fields: vec![value].into(),
        };
    }
    let json = encode(Node::Value(&value));
    let mut node = &json;
    for _ in 0..1000 {
        assert_eq!(node["kind"], "variant");
        assert_eq!(node["name"], "Next");
        assert_eq!(node["fields"].as_array().unwrap().len(), 1);
        node = &node["fields"][0];
    }
    assert_eq!(node["kind"], "option_none");
}

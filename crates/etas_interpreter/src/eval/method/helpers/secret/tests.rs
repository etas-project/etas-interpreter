use super::*;
use crate::{testing::allocation::measure, value::*};

fn secret() -> Value {
    Value::Trust {
        wrapper: etas_types::TrustWrapper::Secret,
        value: Value::String("must-not-leak".into()).into(),
    }
}

fn message(payload: Value) -> MessageValue {
    MessageValue {
        id: String::new(),
        from: None,
        to: None,
        role: MessageRoleValue::User,
        session: None,
        created_at: String::new(),
        provenance: None,
        payload: payload.into(),
    }
}

#[test]
fn iterative_secret_scan_handles_deep_unary_containers_without_allocating() {
    for depth in [1000, 2000, 4000, 30_000] {
        for present in [false, true] {
            let mut value = if present {
                secret()
            } else {
                Value::Bool(false)
            };
            for level in 0..depth {
                value = match level % 8 {
                    0 => Value::Nominal {
                        ty: etas_types::TypeId(0),
                        value: value.into(),
                    },
                    1 => Value::Trust {
                        wrapper: etas_types::TrustWrapper::Trusted,
                        value: value.into(),
                    },
                    2 => Value::OptionSome(value.into()),
                    3 => Value::Message(message(value)),
                    4 => Value::List(vec![value].into()),
                    5 => Value::Record(vec![("field".into(), value)].into()),
                    6 => Value::Array(vec![value].into()),
                    _ => Value::Tuple(vec![value].into()),
                };
            }
            let (found, cost) = measure(|| contains_secret(&value));
            assert_eq!(found, present);
            assert_eq!(cost.count, 0, "depth={depth}: {cost:?}");
            assert_eq!(cost.bytes, 0, "depth={depth}: {cost:?}");
        }
    }
}

#[test]
fn iterative_secret_scan_preserves_nested_sibling_cursors() {
    for depth in [1000, 2000, 4000] {
        for present in [false, true] {
            let mut value = Value::Bool(false);
            for level in 0..depth {
                value = Value::Tuple(
                    vec![
                        value,
                        if present && level == 0 {
                            secret()
                        } else {
                            Value::Unit
                        },
                    ]
                    .into(),
                );
            }
            let (found, cost) = measure(|| contains_secret(&value));
            assert_eq!(found, present);
            assert!(cost.count < 32, "depth={depth}: {cost:?}");
            assert!(cost.bytes < depth * 256, "depth={depth}: {cost:?}");
        }
    }
}

#[test]
fn iterative_secret_scan_covers_each_supported_container_child() {
    for present in [false, true] {
        let child = if present {
            secret()
        } else {
            Value::Bool(false)
        };
        let cases = [
            Value::Stack(vec![Value::Unit, child.clone()].into()),
            Value::Queue(DequeValue::new(vec![Value::Unit, child.clone()])),
            Value::Deque(DequeValue::new(vec![Value::Unit, child.clone()])),
            Value::Set(SetValue::new(vec![Value::Unit, child.clone()])),
            Value::OrderedSet(SetValue::new(vec![Value::Unit, child.clone()])),
            Value::Map(MapValue::new(vec![(child.clone(), Value::Unit)])),
            Value::Map(MapValue::new(vec![(Value::Unit, child.clone())])),
            Value::OrderedMap(MapValue::new(vec![(Value::Unit, child.clone())])),
            Value::PriorityQueue(MapValue::new(vec![(child.clone(), Value::Unit)])),
            Value::Range(RangeValue {
                start: Box::new(child.clone()),
                end: Box::new(Value::Unit),
                bounds: RangeBounds::ClosedOpen,
            }),
            Value::Range(RangeValue {
                start: Box::new(Value::Unit),
                end: Box::new(child.clone()),
                bounds: RangeBounds::ClosedOpen,
            }),
            Value::Variant {
                name: "V".into(),
                fields: vec![Value::Unit, child.clone()].into(),
            },
            Value::Conversation(ConversationValue {
                selected_context: None,
                session: String::new(),
                history_fence: None,
                cursor: None,
                messages: vec![message(Value::Unit), message(child)].into(),
            }),
        ];
        for (index, value) in cases.iter().enumerate() {
            assert_eq!(contains_secret(value), present, "container {index}");
        }
    }
}

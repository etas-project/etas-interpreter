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
fn secret_scan_visits_shared_dag_backing_once() {
    for depth in [16, 1000, 2000, 4000] {
        let mut value = Value::Bool(false);
        for _ in 0..depth {
            value = Value::Array(vec![value.clone(), value].into());
        }
        let value = SharedValue::new(value);
        VISITS.set(0);
        let (found, cost) = measure(|| contains_secret(&value));
        assert!(!found);
        eprintln!(
            "secret DAG depth={depth}: visits={}, {cost:?}",
            VISITS.get()
        );
        assert!(
            VISITS.get() <= 3 * depth + 1,
            "depth={depth}: visits={}",
            VISITS.get()
        );
        assert!(cost.bytes < depth * 256, "depth={depth}: {cost:?}");
        assert_eq!(
            cost.bytes, cost.released_bytes,
            "query cache must be released"
        );
    }
}

#[test]
fn secret_scan_of_one_shared_backing_keeps_zero_allocation_cost() {
    for count in [1000, 2000, 4000] {
        let value = Value::Array(vec![Value::Bool(false); count].into());
        let _alias = value.clone();
        let (found, cost) = measure(|| contains_secret(&value));
        assert!(!found);
        assert_eq!(cost.count, 0, "n={count}: {cost:?}");
        assert_eq!(cost.bytes, 0, "n={count}: {cost:?}");
    }
}

#[test]
fn secret_scan_does_not_rescan_shared_list_suffixes() {
    for count in [1000, 2000, 4000] {
        let mut list = ListValue::new(vec![Value::Bool(false); count]);
        let mut tails = Vec::with_capacity(count);
        while !list.is_empty() {
            tails.push(Value::List(list.clone()));
            assert!(list.advance());
        }
        let value = SharedValue::new(Value::Tuple(tails.into()));
        VISITS.set(0);
        let (found, cost) = measure(|| contains_secret(&value));
        assert!(!found);
        eprintln!(
            "secret List suffixes n={count}: visits={}, {cost:?}",
            VISITS.get()
        );
        assert!(
            VISITS.get() <= 3 * count + 1,
            "n={count}: visits={}",
            VISITS.get()
        );
        assert!(cost.bytes < count * 256, "n={count}: {cost:?}");
        assert_eq!(
            cost.bytes, cost.released_bytes,
            "query cache must be released"
        );
    }
}

#[test]
fn secret_scan_cache_preserves_slice_windows_and_full_array_visibility() {
    let backing = ArrayValue::new(vec![Value::Unit, secret()]);
    let clean = Value::Slice(SliceValue::from_array(backing.clone(), 0..1).unwrap());
    let dirty = Value::Slice(SliceValue::from_array(backing.clone(), 1..2).unwrap());
    assert!(!contains_secret(&Value::Tuple(
        vec![clean.clone(), clean.clone()].into()
    )));
    assert!(contains_secret(&Value::Tuple(
        vec![clean.clone(), dirty].into()
    )));
    assert!(contains_secret(&Value::Tuple(
        vec![clean, Value::Array(backing)].into()
    )));
}

#[test]
fn secret_scan_checks_outer_tags_before_reusing_child_identity() {
    let payload = SharedValue::new(Value::Bool(false));
    let value = Value::Tuple(
        vec![
            Value::OptionSome(payload.clone()),
            Value::Trust {
                wrapper: etas_types::TrustWrapper::Secret,
                value: payload,
            },
        ]
        .into(),
    );
    assert!(contains_secret(&value));

    let tail = ListValue::new(vec![Value::Bool(false); 1000]);
    let mut prefix = tail.clone();
    prefix.push_front(secret());
    let value = Value::Tuple(vec![Value::List(tail), Value::List(prefix)].into());
    assert!(contains_secret(&value));
}

#[test]
fn secret_scan_cache_does_not_survive_a_query_or_hide_later_siblings() {
    let mut value = Value::Bool(false);
    for _ in 0..1000 {
        value = Value::Array(vec![value.clone(), value].into());
    }
    let mut array = ArrayValue::new(vec![value]);
    let original = Value::Array(array.clone());
    assert!(!contains_secret(&original));
    array.borrow_mut().push(secret());
    let changed = Value::Array(array);
    assert!(contains_secret(&Value::Tuple(
        vec![original.clone(), changed].into()
    )));
    assert!(!contains_secret(&original));
    drop(SharedValue::new(original));
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

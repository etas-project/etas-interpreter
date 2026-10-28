use super::*;
use crate::eval::host_value::{interp_to_host_json_string, interp_to_host_value};
use crate::value::{
    ConversationValue, MessageRoleValue, MessageValue, NumericValue, ProvenanceValue,
};
use etas_host::{HostValue, host_value_to_json_string};

#[test]
fn host_wrapper_projection_is_stack_safe_without_intermediate_payload_copies() {
    if crate::testing::in_subprocess("ETAS_TEST_HOST_ABI_WRAPPERS") {
        return;
    }
    let payload = "中😀\n\"payload".repeat(1024);
    let scalar = InterpValue::String(payload.clone().into());
    let (expected_host, host_cost) =
        crate::testing::allocation::measure(|| interp_to_host_value(&scalar).unwrap());
    let (expected_json, json_cost) =
        crate::testing::allocation::measure(|| interp_to_host_json_string(&scalar).unwrap());
    for depth in [1000, 2000, 4000, 30_000] {
        let mut value = scalar.clone();
        for layer in 0..depth {
            value = if layer % 2 == 0 {
                InterpValue::Nominal {
                    ty: etas_types::TypeId(layer as u32),
                    value: crate::value::SharedValue::new(value),
                }
            } else {
                InterpValue::Trust {
                    wrapper: etas_types::TrustWrapper::Public,
                    value: crate::value::SharedValue::new(value),
                }
            };
        }
        let (host, cost) =
            crate::testing::allocation::measure(|| interp_to_host_value(&value).unwrap());
        assert_eq!(host, expected_host);
        assert_eq!(cost.count, host_cost.count, "depth={depth}: {cost:?}");
        assert_eq!(cost.bytes, host_cost.bytes, "depth={depth}: {cost:?}");
        let (json, cost) =
            crate::testing::allocation::measure(|| interp_to_host_json_string(&value).unwrap());
        assert_eq!(json, expected_json);
        assert_eq!(cost.count, json_cost.count, "depth={depth}: {cost:?}");
        assert_eq!(cost.bytes, json_cost.bytes, "depth={depth}: {cost:?}");
        eprintln!("host wrapper ABI depth={depth}: host={host_cost:?}, JSON={cost:?}");
    }
}

#[test]
fn cold_ordered_host_projection_allocates_output_not_a_retained_runtime_view() {
    let mut materialized = Vec::new();
    for count in [1000, 2000, 4000] {
        let reference = InterpValue::Map(
            (0..count)
                .map(|i| (InterpValue::i32(i as i32), InterpValue::Bool(i % 2 == 0)))
                .collect::<Vec<_>>()
                .into(),
        );
        let (reference, output_cost) =
            crate::testing::allocation::measure(|| interp_to_host_value(&reference).unwrap());
        let map = crate::value::OrderedMapValue::try_from_entries(
            (0..count)
                .map(|i| (InterpValue::i32(i as i32), InterpValue::Bool(i % 2 == 0)))
                .collect(),
        )
        .unwrap();
        let value = InterpValue::OrderedMap(map);
        let (output, cost) =
            crate::testing::allocation::measure(|| interp_to_host_value(&value).unwrap());
        assert_eq!(output, reference);
        let HostValue::Map(entries) = &output else {
            panic!("host map")
        };
        assert_eq!(entries.len(), count);
        assert_eq!(
            entries.first(),
            Some(&(HostValue::Int(0), HostValue::Bool(true)))
        );
        assert_eq!(
            entries.last(),
            Some(&(
                HostValue::Int(count as i128 - 1),
                HostValue::Bool((count - 1) % 2 == 0)
            ))
        );
        let output_bytes = entries.capacity() * std::mem::size_of::<(HostValue, HostValue)>();
        let (_, output_drop) = crate::testing::allocation::measure(|| drop(output));
        eprintln!(
            "cold ordered host projection n={count}: {cost:?}; reference_output={output_cost:?}; output_buffer={output_bytes}; release={output_drop:?}"
        );
        if cost.bytes > output_cost.bytes + 4096
            || cost.bytes - cost.released_bytes != output_drop.released_bytes
        {
            materialized.push((count, cost, output_drop));
        }
    }
    assert!(
        materialized.is_empty(),
        "host projection retained runtime views: {materialized:?}"
    );
}

#[test]
fn borrowed_projection_preserves_scalar_aggregate_nominal_and_trust_encoding() {
    let values = vec![
        InterpValue::Unit,
        InterpValue::Bool(true),
        InterpValue::Number(NumericValue::I128(i64::MIN as i128)),
        InterpValue::Number(NumericValue::U128(u64::MAX as u128)),
        InterpValue::Number(NumericValue::F64((-0.0_f64).to_bits())),
        InterpValue::String("λ\n\"\\".into()),
        InterpValue::Bytes(vec![0, 255].into()),
        InterpValue::Tuple(vec![InterpValue::Bool(true)].into()),
        InterpValue::Array(vec![InterpValue::String("array".into())].into()),
        InterpValue::List(vec![InterpValue::String("list".into())].into()),
        InterpValue::Deque(vec![InterpValue::String("deque".into())].into()),
        InterpValue::Queue(vec![InterpValue::String("queue".into())].into()),
        InterpValue::Stack(vec![InterpValue::String("stack".into())].into()),
        InterpValue::Map(vec![(InterpValue::String("key".into()), InterpValue::i32(1))].into()),
        InterpValue::Record(
            vec![
                ("z".into(), InterpValue::Unit),
                ("a".into(), InterpValue::Bool(false)),
            ]
            .into(),
        ),
        InterpValue::Variant {
            name: "Present".into(),
            fields: vec![InterpValue::i32(42)].into(),
        },
        InterpValue::OptionNone,
        InterpValue::OptionSome(crate::value::SharedValue::new(InterpValue::String(
            "value".into(),
        ))),
        InterpValue::Json(HostJsonSupportValue::Object(
            vec![
                (
                    "z".into(),
                    HostJsonSupportValue::Array(
                        vec![HostJsonSupportValue::Null, HostJsonSupportValue::Bool(true)].into(),
                    ),
                ),
                (
                    "a".into(),
                    HostJsonSupportValue::NumberBits(1.25_f64.to_bits()),
                ),
            ]
            .into(),
        )),
    ];
    for value in values {
        let encoded = interp_to_host_json_string(&value).unwrap();
        assert_eq!(
            encoded,
            host_value_to_json_string(&interp_to_host_value(&value).unwrap()).unwrap()
        );
        let value = InterpValue::Nominal {
            ty: etas_types::TypeId(0),
            value: crate::value::SharedValue::new(value),
        };
        assert_eq!(encoded, interp_to_host_json_string(&value).unwrap());
        let value = InterpValue::Trust {
            wrapper: etas_types::TrustWrapper::Public,
            value: crate::value::SharedValue::new(value),
        };
        assert_eq!(encoded, interp_to_host_json_string(&value).unwrap());
    }
}

fn message() -> MessageValue {
    MessageValue {
        id: "m1".into(),
        from: Some("sender".into()),
        to: None,
        role: MessageRoleValue::Assistant,
        session: Some("s1".into()),
        created_at: "now".into(),
        payload: crate::value::SharedValue::new(InterpValue::Array(
            vec![InterpValue::String("payload".into())].into(),
        )),
        provenance: Some(ProvenanceValue {
            trace_id: Some("trace".into()),
            source: None,
        }),
    }
}

#[test]
fn message_projection_matches_canonical_envelope_and_conversation_context() {
    let message = message();
    let envelope =
        crate::eval::boundary_session::message_envelope_from_message_value(&message).unwrap();
    let expected_message = etas_host::session::message_envelope_to_host_value(&envelope);
    let value = InterpValue::Message(message.clone());
    assert_eq!(interp_to_host_value(&value).unwrap(), expected_message);
    assert_eq!(
        interp_to_host_json_string(&value).unwrap(),
        host_value_to_json_string(&expected_message).unwrap()
    );
    let value = InterpValue::Conversation(ConversationValue {
        session: "s1".into(),
        history_fence: None,
        messages: vec![message].into(),
        cursor: Some("cursor".into()),
        selected_context: None,
    });
    let expected = HostValue::Record(vec![
        ("selected_context".into(), HostValue::Unit),
        ("history_fence".into(), HostValue::Unit),
        ("session".into(), HostValue::String("s1".into())),
        ("messages".into(), HostValue::List(vec![expected_message])),
        ("cursor".into(), HostValue::String("cursor".into())),
    ]);
    assert_eq!(interp_to_host_value(&value).unwrap(), expected);
    assert_eq!(
        interp_to_host_json_string(&value).unwrap(),
        host_value_to_json_string(&expected).unwrap()
    );
}

#[test]
fn invalid_projection_and_json_values_fail_without_partial_output() {
    for value in [
        InterpValue::Number(NumericValue::U128(u128::MAX)),
        InterpValue::Number(NumericValue::I128(i128::MIN)),
        InterpValue::Number(NumericValue::F64(f64::NAN.to_bits())),
        InterpValue::Json(HostJsonSupportValue::NumberBits(f64::INFINITY.to_bits())),
        InterpValue::Record(
            vec![
                ("a".into(), InterpValue::Unit),
                ("a".into(), InterpValue::Unit),
            ]
            .into(),
        ),
        InterpValue::Json(HostJsonSupportValue::Object(
            vec![
                ("a".into(), HostJsonSupportValue::Null),
                ("a".into(), HostJsonSupportValue::Null),
            ]
            .into(),
        )),
        InterpValue::Slice(vec![InterpValue::i32(1)].into()),
        InterpValue::Set(vec![InterpValue::i32(1)].into()),
    ] {
        let reference = interp_to_host_value(&value)
            .and_then(|v| host_value_to_json_string(&v).map_err(|error| error.message))
            .unwrap_err();
        let wrapped =
            InterpValue::Array(vec![InterpValue::String("valid prefix".into()), value].into());
        let error = interp_to_host_json_string(&wrapped).unwrap_err();
        assert_eq!(error.code, HostErrorCode::SchemaMismatch);
        assert_eq!(error.message, reference);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_projection_preserves_selected_context_evidence() {
    use etas_host::{SessionClient, SessionOperation, SessionResult};
    let client = etas_host::InMemorySessionClient::new();
    let request = |operation| etas_host::SessionRequest {
        id: etas_host::HostRequestId(1),
        operation,
        authority: etas_host::AuthorityContext::deny_all(),
        trace: etas_host::TraceContext::root(etas_host::TraceId(1)),
        budget: etas_host::ExecutionBudget::default(),
    };
    client
        .execute(request(SessionOperation::Resolve {
            config: etas_host::SessionConfig {
                id: "s".into(),
                context: etas_host::ContextPolicy::All,
                retention: etas_host::RetentionPolicy::Forever,
            },
        }))
        .await
        .unwrap()
        .result
        .unwrap();
    let SessionResult::History { fence, .. } = client
        .execute(request(SessionOperation::Load {
            session: etas_host::SessionRef { id: "s".into() },
            context: etas_host::ContextPolicy::All,
            cursor: None,
            limit: Some(1),
        }))
        .await
        .unwrap()
        .result
        .unwrap()
    else {
        panic!("history")
    };
    let token = fence.as_token().to_owned();
    let value = InterpValue::Conversation(ConversationValue {
        session: "s".into(),
        history_fence: Some(fence.clone()),
        messages: vec![].into(),
        cursor: None,
        selected_context: Some(
            etas_host::session::SessionPublishedContext {
                content: etas_host::session::SessionContextContent {
                    text: "selected text".into(),
                    provenance: [("producer".into(), "source-v2".into())].into(),
                },
                fence,
                version: 7,
            }
            .into(),
        ),
    });
    let encoded = interp_to_host_json_string(&value).unwrap();
    let json: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "session": "s", "messages": [], "cursor": null, "history_fence": token,
            "selected_context": { "text": "selected text", "provenance": {"producer": "source-v2"}, "fence": token, "version": 7 },
        })
    );
    assert_eq!(
        encoded,
        host_value_to_json_string(&interp_to_host_value(&value).unwrap()).unwrap()
    );
}

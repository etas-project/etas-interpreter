use super::*;
use crate::{
    orchestration::ValueSnapshot,
    value::{InterpValue, MessageRoleValue, MessageValue, ProvenanceValue},
};

fn view() -> ConversationValue {
    ConversationValue {
        selected_context: None,
        history_fence: None,
        cursor: None,
        session: "session".into(),
        messages: vec![MessageValue {
            id: "id".into(),
            from: None,
            to: None,
            role: MessageRoleValue::User,
            session: Some("session".into()),
            created_at: "time".into(),
            payload: Box::new(InterpValue::String("data".into())),
            provenance: None,
        }],
    }
}

fn check_all(value: &ConversationValue, limits: &StorageLimits, valid: bool) {
    assert_eq!(validate(value, limits).is_ok(), valid, "runtime");
    let ValueSnapshot::Conversation(snapshot) =
        ValueSnapshot::capture(&InterpValue::Conversation(value.clone())).unwrap()
    else {
        panic!("conversation");
    };
    assert_eq!(
        validate_snapshot(&snapshot, limits).is_ok(),
        valid,
        "snapshot"
    );
    let encoded = crate::api::codec::value_json(&InterpValue::Conversation(value.clone()));
    assert_eq!(
        validate_json(&encoded, limits).is_ok(),
        valid,
        "borrowed JSON"
    );
    assert_eq!(
        crate::api::codec::value_from_json_with_limits(limits, &encoded).is_ok(),
        valid,
        "decode"
    );
}

#[test]
fn storage_view_limits_cover_envelope_payload_provenance_count_bytes_nodes_and_depth() {
    let limits = StorageLimits {
        max_value_bytes: 1024,
        max_result_bytes: 4096,
        ..Default::default()
    };
    check_all(&view(), &limits, true);
    for field in ["payload", "id", "from", "to", "time", "provenance"] {
        let mut value = view();
        let large = "x".repeat(2048);
        let message = &mut value.messages[0];
        match field {
            "payload" => *message.payload = InterpValue::String(large),
            "id" => message.id = large,
            "from" => message.from = Some(large),
            "to" => message.to = Some(large),
            "time" => message.created_at = large,
            "provenance" => {
                message.provenance = Some(ProvenanceValue {
                    trace_id: None,
                    source: Some(large),
                })
            }
            _ => unreachable!(),
        }
        check_all(&value, &limits, false);
    }
    let mut value = view();
    value.messages[0].from = Some("x".repeat(550));
    value.messages[0].to = Some("x".repeat(550));
    check_all(&value, &limits, false);
    let mut value = view();
    value.messages = vec![value.messages[0].clone(); 30];
    check_all(
        &value,
        &StorageLimits {
            max_scan_rows: 10,
            ..limits.clone()
        },
        false,
    );
    check_all(
        &value,
        &StorageLimits {
            max_result_bytes: 1024,
            ..limits.clone()
        },
        false,
    );
    let mut value = view();
    *value.messages[0].payload = InterpValue::Array(vec![InterpValue::Bool(true); 20].into());
    check_all(
        &value,
        &StorageLimits {
            max_nodes: 10,
            ..limits.clone()
        },
        false,
    );
    let mut value = view();
    for _ in 0..20 {
        *value.messages[0].payload = InterpValue::OptionSome(value.messages[0].payload.clone());
    }
    check_all(
        &value,
        &StorageLimits {
            max_depth: 8,
            ..limits
        },
        false,
    );
}

#[test]
fn json_preflight_rejects_size_before_decoding_payloads() {
    let mut value = view();
    *value.messages[0].payload = InterpValue::String("x".repeat(65536));
    let mut encoded = crate::api::codec::value_json(&InterpValue::Conversation(value));
    // A later invalid runtime value must never be reached before the size rejection.
    let mut bad = encoded["messages"][0].clone();
    bad["payload"] = serde_json::json!({"kind":"number", "type":"invalid"});
    encoded["messages"].as_array_mut().unwrap().push(bad);
    let limits = StorageLimits {
        max_value_bytes: 8192,
        ..Default::default()
    };
    let error = crate::api::codec::value_from_json_with_limits(&limits, &encoded).unwrap_err();
    assert!(error.message().contains("storage resource limit"));
}

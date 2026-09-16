use crate::{
    orchestration::{ConversationSnapshot, MessageSnapshot, SnapshotBox, ValueSnapshot},
    value::*,
};

fn message(value: InterpValue) -> MessageValue {
    MessageValue {
        id: String::new(),
        from: None,
        to: None,
        role: MessageRoleValue::User,
        session: None,
        created_at: String::new(),
        payload: value.into(),
        provenance: None,
    }
}

fn snapshot_message(value: ValueSnapshot) -> MessageSnapshot {
    MessageSnapshot {
        id: String::new(),
        from: None,
        to: None,
        role: MessageRoleValue::User,
        session: None,
        created_at: String::new(),
        payload: SnapshotBox::new(value),
        provenance: None,
    }
}

fn runtime_chain(depth: usize) -> InterpValue {
    (0..depth).fold(InterpValue::Bool(true), |value, i| {
        if i % 2 == 0 {
            InterpValue::Message(message(value))
        } else {
            InterpValue::Conversation(ConversationValue {
                selected_context: None,
                session: String::new(),
                history_fence: None,
                cursor: None,
                messages: vec![message(value)].into(),
            })
        }
    })
}

fn snapshot_chain(depth: usize) -> ValueSnapshot {
    (0..depth).fold(ValueSnapshot::Bool(true), |value, i| {
        if i % 2 == 0 {
            ValueSnapshot::Message(snapshot_message(value))
        } else {
            ValueSnapshot::Conversation(ConversationSnapshot {
                selected_context: None,
                session: String::new(),
                history_fence: None,
                cursor: None,
                messages: vec![snapshot_message(value)],
            })
        }
    })
}

fn subprocess(worker: &str) -> bool {
    if std::env::var_os(worker).is_some() {
        return false;
    }
    let thread = std::thread::current();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", thread.name().unwrap(), "--nocapture"])
        .env(worker, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[test]
fn deep_message_capture_uses_the_snapshot_worklist() {
    if subprocess("ETAS_DEEP_MESSAGE_CAPTURE_WORKER") {
        return;
    }
    let runtime = runtime_chain(30_000);
    let snapshot = ValueSnapshot::capture(&runtime).unwrap();
    assert!(snapshot == snapshot_chain(30_000));
    drop(runtime);
    drop(snapshot);
}

#[test]
fn deep_message_restore_uses_the_snapshot_worklist() {
    if subprocess("ETAS_DEEP_MESSAGE_RESTORE_WORKER") {
        return;
    }
    let snapshot = snapshot_chain(30_000);
    let restored = snapshot.restore().unwrap();
    assert!(restored == runtime_chain(30_000));
    drop(restored);
}

#[test]
fn conversation_capture_restore_preserves_message_order_and_shared_byte_payloads() {
    use crate::testing::allocation::measure;
    for count in [1000, 2000, 4000] {
        let bytes = BytesValue::from(vec![7; 4096]);
        let pointer = bytes.as_ptr();
        let runtime = InterpValue::Conversation(ConversationValue {
            selected_context: None,
            session: String::new(),
            history_fence: None,
            cursor: None,
            messages: (0..count)
                .map(|i| {
                    let mut message = message(InterpValue::Bytes(bytes.clone()));
                    message.role = match i % 4 {
                        0 => MessageRoleValue::System,
                        1 => MessageRoleValue::User,
                        2 => MessageRoleValue::Assistant,
                        _ => MessageRoleValue::Tool,
                    };
                    message
                })
                .collect(),
        });
        let (captured, capture_cost) = measure(|| ValueSnapshot::capture(&runtime).unwrap());
        eprintln!("conversation capture n={count}: {capture_cost:?}");
        // One final message vector, one context header, and one payload node per message.
        assert_eq!(capture_cost.count, count + 2);
        let ValueSnapshot::Conversation(ref view) = captured else {
            panic!("conversation")
        };
        assert_eq!(view.messages.len(), count);
        for message in &view.messages {
            let ValueSnapshot::Bytes(bytes) = message.payload.as_ref() else {
                panic!("bytes")
            };
            assert_eq!(bytes.as_ptr(), pointer);
        }
        let (restored, restore_cost) = measure(|| captured.restore().unwrap());
        eprintln!("conversation restore n={count}: {restore_cost:?}");
        // The runtime adds one immutable MessageList owner to the same traversal shape.
        assert_eq!(restore_cost.count, count + 3);
        assert!(restored == runtime);
        let InterpValue::Conversation(restored) = restored else {
            panic!("conversation")
        };
        for message in &restored.messages {
            let InterpValue::Bytes(bytes) = message.payload.as_ref() else {
                panic!("bytes")
            };
            assert_eq!(bytes.as_ptr(), pointer);
        }
    }
}

#[test]
fn capture_failure_releases_deep_partial_conversation_without_copying_runtime() {
    if subprocess("ETAS_PARTIAL_MESSAGE_CAPTURE_WORKER") {
        return;
    }
    use crate::testing::allocation::measure;
    let handle = InterpValue::HostHandle(HostHandleValue::tcp_stream(
        etas_types::TypeId(1),
        etas_host::TcpStreamRef::issued(
            etas_host::StreamHandleRef::issued("live", 0),
            etas_host::ByteStreamOrigin::Tcp {
                host: "example.test".into(),
                port: 443,
            },
        ),
    ));
    let runtime = InterpValue::Conversation(ConversationValue {
        selected_context: None,
        session: String::new(),
        history_fence: None,
        cursor: None,
        messages: vec![message(runtime_chain(30_000)), message(handle)].into(),
    });
    let (_, cost) = measure(|| {
        let error = ValueSnapshot::capture(&runtime).unwrap_err();
        assert!(error.contains("live tcp_stream host handles"), "{error}");
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "partial snapshot leaked: {cost:?}"
    );
    let InterpValue::Conversation(view) = runtime else {
        panic!("conversation")
    };
    assert_eq!(view.messages.len(), 2);
    assert!(view.messages[0].payload.as_ref() == &runtime_chain(30_000));
}

#[test]
fn restore_failure_releases_deep_partial_conversation() {
    if subprocess("ETAS_PARTIAL_MESSAGE_RESTORE_WORKER") {
        return;
    }
    use crate::testing::allocation::measure;
    let (_, cost) = measure(|| {
        let snapshot = ValueSnapshot::Conversation(ConversationSnapshot {
            selected_context: None,
            session: String::new(),
            history_fence: None,
            cursor: None,
            messages: vec![
                snapshot_message(snapshot_chain(30_000)),
                snapshot_message(ValueSnapshot::Set(
                    vec![ValueSnapshot::Bool(true), ValueSnapshot::Bool(true)].into(),
                )),
            ],
        });
        let error = snapshot.restore().unwrap_err();
        assert!(error.contains("duplicate"), "{error}");
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "partial runtime leaked: {cost:?}"
    );
}

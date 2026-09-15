use super::*;
use crate::{control::Frame, plan::SlotLayoutTable, testing::allocation::measure};
use etas_hir::SymbolId;
use std::sync::Arc;

fn message(payload: InterpValue) -> MessageValue {
    MessageValue {
        id: String::new(),
        from: None,
        to: None,
        role: MessageRoleValue::User,
        session: None,
        created_at: String::new(),
        payload: payload.into(),
        provenance: None,
    }
}

fn conversation(messages: Vec<MessageValue>) -> InterpValue {
    InterpValue::Conversation(ConversationValue {
        selected_context: None,
        session: String::new(),
        history_fence: None,
        messages: messages.into(),
        cursor: None,
    })
}

#[test]
fn conversation_frame_reads_share_messages_and_payloads() {
    for count in [1000, 2000, 4000] {
        let original = conversation(
            (0..count)
                .map(|i| {
                    let mut message = message(InterpValue::Bytes(vec![7; 4096].into()));
                    message.id = format!("message-{i}");
                    message.from = Some("sender".repeat(32));
                    message.provenance = Some(ProvenanceValue {
                        trace_id: Some("trace-id".into()),
                        source: Some("source".repeat(32)),
                    });
                    message
                })
                .collect(),
        );
        let mut frame = Frame::new(Arc::new(SlotLayoutTable::from_symbols(vec![SymbolId(0)])));
        frame.insert(SymbolId(0), original);
        let (alias, cost) = measure(|| frame.get(SymbolId(0)).unwrap());
        eprintln!("conversation frame read n={count}: {cost:?}");
        assert_eq!(
            cost.count, 0,
            "message headers/payloads were copied: {cost:?}"
        );
        let InterpValue::Conversation(alias) = alias else {
            panic!("conversation")
        };
        assert_eq!(alias.messages.len(), count);
    }
}

#[test]
fn nested_message_aliases_do_not_copy_payload_nodes() {
    for depth in [16, 32, 64] {
        let original = (0..depth).fold(InterpValue::Unit, |value, _| {
            InterpValue::Message(message(value))
        });
        let (alias, cost) = measure(|| original.clone());
        eprintln!("message alias depth={depth}: {cost:?}");
        assert_eq!(cost.count, 0, "payload chain was copied: {cost:?}");
        assert_eq!(original, alias);
    }
}

#[test]
fn nested_message_conversation_alias_and_release_are_stack_safe() {
    const WORKER: &str = "ETAS_MESSAGE_LIFECYCLE_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let thread = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", thread.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let chain = || {
        (0..30_000).fold(InterpValue::Unit, |value, i| {
            if i % 2 == 0 {
                InterpValue::Message(message(value))
            } else {
                conversation(vec![message(value)])
            }
        })
    };
    let original = chain();
    let alias = original.clone();
    let independent = chain();
    assert!(alias == independent);
    drop(original);
    assert!(alias == independent);
    drop(alias);
    drop(independent);
}

#[test]
fn message_extraction_moves_unique_storage_and_isolates_live_aliases() {
    for count in [1000, 2000, 4000] {
        let original = MessageList::from(
            (0..count)
                .map(|_| message(InterpValue::Bytes(vec![7; 4096].into())))
                .collect::<Vec<_>>(),
        );
        let original_ptr = original.as_ptr();
        let (mut extracted, cost) = measure(|| original.clone().into_messages());
        assert_eq!(cost.count, 1, "{cost:?}");
        assert_eq!(cost.bytes, count * std::mem::size_of::<MessageValue>());
        assert_ne!(extracted.as_ptr(), original_ptr);
        assert!(std::ptr::eq(
            extracted[0].payload.as_ref(),
            original[0].payload.as_ref()
        ));
        extracted[0].payload = InterpValue::Bool(false).into();
        assert!(matches!(
            original[0].payload.as_ref(),
            InterpValue::Bytes(_)
        ));
        let (moved, cost) = measure(|| original.into_messages());
        assert_eq!(cost.count, 0);
        assert_eq!(moved.as_ptr(), original_ptr);
    }
}

#[test]
fn conversation_equality_checks_headers_order_and_payloads_without_copying() {
    let a = conversation(vec![
        message(InterpValue::Bool(true)),
        message(InterpValue::Bool(false)),
    ]);
    let b = conversation(vec![
        message(InterpValue::Bool(true)),
        message(InterpValue::Bool(false)),
    ]);
    assert_eq!(a, b);
    let InterpValue::Conversation(mut changed) = b else {
        panic!("conversation")
    };
    let mut messages = changed.messages.into_messages();
    messages.swap(0, 1);
    changed.messages = messages.into();
    assert_ne!(a, InterpValue::Conversation(changed.clone()));
    let mut messages = changed.messages.into_messages();
    messages.swap(0, 1);
    messages[0].provenance = Some(ProvenanceValue {
        source: Some("different".into()),
        trace_id: None,
    });
    changed.messages = messages.into();
    assert_ne!(a, InterpValue::Conversation(changed));
    let (equal, cost) = measure(|| a == a.clone());
    assert!(equal);
    assert_eq!(cost.count, 0);
}

#[test]
fn shared_message_release_reclaims_owned_nodes_but_preserves_retained_subtrees() {
    let (_, cost) = measure(|| {
        let mut value = InterpValue::Bool(true);
        for _ in 0..4000 {
            value = conversation(vec![message(value)]);
        }
        let retained = message(value);
        let root = conversation(vec![retained.clone(), retained.clone()]);
        drop(root);
        assert!(matches!(
            retained.payload.as_ref(),
            InterpValue::Conversation(_)
        ));
        drop(retained);
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "shared message graph leaked: {cost:?}"
    );
}

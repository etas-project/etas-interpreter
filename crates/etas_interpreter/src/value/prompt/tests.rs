use super::*;
use crate::{
    orchestration::ValueSnapshot,
    testing::allocation::measure,
    value::{InterpValue, PromptRole},
};

fn message(text: impl Into<StringValue>) -> PromptMessage {
    PromptMessage {
        role: PromptRole::User,
        text: text.into(),
        trust: None,
    }
}

#[test]
fn shared_single_message_text_keeps_backing_and_snapshot_isolation() {
    for count in [1000, 2000, 4000] {
        let payload = "x".repeat(count * 1024);
        let pointer = payload.as_ptr();
        let value = InterpValue::Prompt(PromptValue::new(vec![message(payload)]));
        let saved = ValueSnapshot::capture(&value).unwrap();
        let InterpValue::Prompt(prompt) = value else {
            unreachable!()
        };
        let (mut text, cost) = measure(|| prompt.into_text());
        assert_eq!(cost.count, 0, "n={count}: {cost:?}");
        assert_eq!(cost.bytes, 0, "n={count}: {cost:?}");
        assert_eq!(text.as_ptr(), pointer);
        text.push_str(" changed");
        let ValueSnapshot::Prompt(saved) = saved else {
            unreachable!()
        };
        assert_eq!(saved[0].text.as_ptr(), pointer);
        assert_eq!(saved[0].text.len(), count * 1024);
        assert_eq!(text.len(), count * 1024 + 8);
        let (_, released) = measure(|| drop(saved));
        assert!(released.released_bytes >= count * 1024, "{released:?}");
    }
}

#[test]
fn aliases_and_snapshots_share_immutable_messages_without_payload_copies() {
    for count in [1000, 2000, 4000] {
        let value = InterpValue::Prompt((0..count).map(|_| message("x".repeat(1024))).collect());
        let InterpValue::Prompt(original) = &value else {
            unreachable!()
        };
        // Reference the former Vec backing, whose message text was owned String.
        let previous: Vec<_> = original
            .iter()
            .map(|message| {
                (
                    message.role,
                    message.text.as_str().to_owned(),
                    message.trust,
                )
            })
            .collect();
        let (previous_clone, before) = measure(|| previous.clone());
        assert_eq!(previous_clone, previous);
        assert!(before.bytes >= count * 1024);
        let ((alias, snapshot, restored), allocations) = measure(|| {
            let alias = value.clone();
            let snapshot = ValueSnapshot::capture(&value).unwrap();
            let restored = snapshot.clone().restore().unwrap();
            (alias, snapshot, restored)
        });
        assert_eq!(
            allocations.count, 0,
            "Prompt sharing allocated: {allocations:?}"
        );
        let InterpValue::Prompt(original) = &value else {
            unreachable!()
        };
        let InterpValue::Prompt(mut changed) = alias else {
            unreachable!()
        };
        let InterpValue::Prompt(restored) = restored else {
            unreachable!()
        };
        let ValueSnapshot::Prompt(saved) = snapshot else {
            unreachable!()
        };
        assert_eq!(original.as_ptr(), changed.as_ptr());
        assert_eq!(original.as_ptr(), restored.as_ptr());
        assert_eq!(original.as_ptr(), saved.as_ptr());
        let addition = message("new");
        let (_, write) = measure(|| changed.push(addition));
        assert_ne!(original.as_ptr(), changed.as_ptr());
        assert_eq!(changed.len(), count + 1);
        assert_eq!(saved.len(), count);
        assert_eq!(restored.len(), count);
        assert_eq!(original[0].text.as_ptr(), changed[0].text.as_ptr());
        assert!(
            write.bytes <= (count + 1) * std::mem::size_of::<PromptMessage>() + 256,
            "shared update copied payload or allocated a second table: {write:?}"
        );
        eprintln!(
            "prompt n={count}: former-clone={before:?}; share={allocations:?}; shared-append={write:?}"
        );
        let mut changed = changed.into_values();
        changed[0].text.push_str("modified");
        assert_eq!(original[0].text.len(), 1024);
        assert_eq!(saved[0].text.len(), 1024);
        assert_eq!(restored[0].text.len(), 1024);
    }
}

#[test]
fn unique_update_and_consuming_extraction_reuse_the_message_table() {
    let mut messages = Vec::with_capacity(3);
    messages.push(message("first"));
    let original = messages.as_ptr();
    let mut prompt = PromptValue::new(messages);
    let second = message("second");
    let (messages, allocations) = measure(|| {
        prompt.push(second);
        prompt.into_values()
    });
    assert_eq!(allocations.count, 0, "{allocations:?}");
    assert_eq!(messages.as_ptr(), original);
    assert_eq!(messages[0].text, "first");
    assert_eq!(messages[1].text, "second");
}

#[test]
fn owned_and_shared_prompt_text_keep_identical_separator_semantics() {
    for parts in [
        vec![],
        vec![""],
        vec!["a"],
        vec!["中😀e\u{301}"],
        vec!["中", "", "😀e\u{301}"],
        vec!["", "b", ""],
        vec!["a", "b", "c"],
    ] {
        let expected = parts.join("\n");
        let prompt: PromptValue = parts.iter().map(|text| message(*text)).collect();
        let alias = prompt.clone();
        assert_eq!(prompt.into_text(), expected);
        assert_eq!(alias.len(), parts.len());
        for (message, expected) in alias.iter().zip(&parts) {
            assert_eq!(&message.text, *expected);
        }
        assert_eq!(alias.into_text(), expected);
    }
    let mut text = String::with_capacity(128);
    text.push_str("first");
    let original = text.as_ptr();
    let prompt = PromptValue::new(vec![message(text), message("second")]);
    let (text, allocations) = measure(|| prompt.into_text());
    assert_eq!(text, "first\nsecond");
    assert_eq!(text.as_ptr(), original);
    assert_eq!(
        allocations.count, 0,
        "unique text backing should be reused: {allocations:?}"
    );
}

use super::*;
use crate::testing::allocation::measure;
use crate::value::PromptRole;
use etas_builtin::text::input::TextInput;

fn cold(prefix: String, suffix: &str) -> StringValue {
    let prefix = StringValue::from(prefix);
    let alias = prefix.clone();
    let value = prefix.concat(suffix.into()).unwrap();
    drop(alias);
    assert!(value.contiguous().is_none());
    value
}

fn message(text: StringValue) -> PromptMessage {
    PromptMessage {
        role: PromptRole::User,
        text,
        trust: None,
    }
}

#[test]
fn prompt_text_assembly_shares_cold_messages_and_counts_final_output_separately() {
    let mut results = Vec::new();
    for count in [1000, 2000, 4000] {
        for shared in [false, true] {
            let first = cold("first中😀".repeat(count), "first-tail");
            let second = cold("second中😀".repeat(count), "second-tail");
            let prompt = PromptValue::new(vec![message(first.clone()), message(second.clone())]);
            let alias = shared.then(|| prompt.clone());
            let (text, assembly) = measure(|| prompt.into_text().unwrap());
            let cached = first.contiguous().is_some() || second.contiguous().is_some();
            let (output, output_cost) = measure(|| text.into_string());
            assert_eq!(
                output,
                format!(
                    "{}first-tail\n{}second-tail",
                    "first中😀".repeat(count),
                    "second中😀".repeat(count)
                )
            );
            if let Some(alias) = alias {
                assert_eq!(alias[0].text, first);
                assert_eq!(alias[1].text, second);
            }
            eprintln!(
                "cold Prompt.text n={count}, shared={shared}: assembly={assembly:?}, final-output={output_cost:?}, cached={cached}"
            );
            results.push((count, shared, cached, assembly));
        }
    }
    for (count, shared, cached, cost) in results {
        assert!(
            !cached,
            "assembly flattened a message: n={count}, shared={shared}, {cost:?}"
        );
        assert!(
            cost.bytes < 1024,
            "assembly copied payload: n={count}, shared={shared}, {cost:?}"
        );
    }
}

#[test]
fn prompt_text_assembly_rejects_length_overflow_without_expanding_shared_inputs() {
    let mut text = StringValue::from("x");
    for _ in 0..isize::BITS - 2 {
        text = text.clone().concat(text).unwrap();
    }
    let prompt = PromptValue::new(vec![message(text.clone()), message(text.clone())]);
    let (result, cost) = measure(|| prompt.into_text());
    assert!(result.is_err());
    assert_eq!(cost.count, 0);
    assert!(text.contiguous().is_none());
}

#[test]
fn prompt_text_assembly_scales_with_message_headers_without_copying_payloads() {
    let mut previous = None;
    for count in [1000, 2000, 4000] {
        let text = cold("message中😀".repeat(128), "tail");
        let prompt = PromptValue::new((0..count).map(|_| message(text.clone())).collect());
        let saved = prompt.clone();
        let (result, assembly) = measure(|| prompt.into_text().unwrap());
        assert!(text.contiguous().is_none());
        assert!(
            assembly.bytes <= count * 256,
            "payload-size assembly n={count}: {assembly:?}"
        );
        if let Some(bytes) = previous {
            assert!(assembly.bytes <= bytes * 2 + 256);
        }
        previous = Some(assembly.bytes);
        let (output, output_cost) = measure(|| result.into_string());
        assert_eq!(
            output,
            vec![format!("{}tail", "message中😀".repeat(128)); count].join("\n")
        );
        assert_eq!(saved.len(), count);
        assert!(text.contiguous().is_none());
        eprintln!(
            "Prompt.text messages={count}: assembly={assembly:?}, final-output={output_cost:?}"
        );
    }
}

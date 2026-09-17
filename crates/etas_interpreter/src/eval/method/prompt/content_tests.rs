use crate::{testing::allocation::measure, value::*};
use etas_core::AnalysisDiagnosticCode;
use etas_types::TrustWrapper;

fn message(payload: InterpValue, metadata: String) -> InterpValue {
    InterpValue::Message(MessageValue {
        id: metadata,
        from: None,
        to: None,
        role: MessageRoleValue::User,
        session: None,
        created_at: String::new(),
        provenance: None,
        payload: payload.into(),
    })
}

fn wrapped(wrapper: TrustWrapper, value: InterpValue) -> InterpValue {
    InterpValue::Trust {
        wrapper,
        value: value.into(),
    }
}

#[test]
fn prompt_text_projection_does_not_copy_shared_message_metadata() {
    super::tests::with_prompt_eval(|eval, span| {
        for count in [1000, 2000, 4000] {
            let input = wrapped(
                TrustWrapper::Trusted,
                message(
                    InterpValue::String("中😀e\u{301}".into()),
                    "m".repeat(count * 1024),
                ),
            );
            let saved = crate::orchestration::ValueSnapshot::capture(&input).unwrap();
            let (result, cost) = measure(|| {
                eval.prompt_channel_content("system", input.clone(), span, false)
                    .unwrap()
            });
            assert_eq!(result.0, "中😀e\u{301}");
            assert_eq!(result.1, Some(TrustWrapper::Trusted));
            assert_eq!(cost.count, 0, "n={count}: {cost:?}");
            assert_eq!(cost.bytes, 0, "n={count}: {cost:?}");
            assert_eq!(saved.restore().unwrap(), input);
        }
    });
}

#[test]
fn prompt_text_projection_is_iterative_for_owned_and_shared_wrapper_chains() {
    super::tests::with_prompt_eval(|eval, span| {
        for depth in [1000, 2000, 4000, 30_000] {
            for shared in [false, true] {
                let mut input = InterpValue::String("中😀e\u{301}".into());
                for index in 0..depth {
                    input = if index % 2 == 0 {
                        wrapped(TrustWrapper::Trusted, input)
                    } else {
                        message(input, String::new())
                    };
                }
                let alias = shared.then(|| input.clone());
                let (result, cost) = measure(|| {
                    eval.prompt_channel_content("system", input, span, false)
                        .unwrap()
                });
                assert_eq!(result.0, "中😀e\u{301}");
                assert_eq!(result.1, Some(TrustWrapper::Trusted));
                assert_eq!(cost.count, 0, "depth={depth} shared={shared}: {cost:?}");
                assert_eq!(cost.bytes, 0, "depth={depth} shared={shared}: {cost:?}");
                drop(alias);
            }
        }
    });
}

#[test]
fn prompt_text_projection_checks_every_wrapper_and_keeps_outer_trust() {
    super::tests::with_prompt_eval(|eval, span| {
        for outer in [TrustWrapper::Trusted, TrustWrapper::Untrusted] {
            for inner in [
                TrustWrapper::Trusted,
                TrustWrapper::Untrusted,
                TrustWrapper::Secret,
            ] {
                for method in ["user", "assistant", "system"] {
                    for allow_plain in [false, true] {
                        for shared in [false, true] {
                            let input = wrapped(
                                outer,
                                message(
                                    wrapped(inner, InterpValue::String("sensitive payload".into())),
                                    String::new(),
                                ),
                            );
                            let _alias = shared.then(|| input.clone());
                            let result =
                                eval.prompt_channel_content(method, input, span, allow_plain);
                            if inner == TrustWrapper::Secret
                                || (method == "system"
                                    && (outer != TrustWrapper::Trusted
                                        || inner != TrustWrapper::Trusted))
                            {
                                let error = result.unwrap_err();
                                assert_eq!(error.code, AnalysisDiagnosticCode::InvalidArguments);
                                assert!(!error.message.contains("sensitive payload"));
                            } else {
                                let (text, trust) = result.unwrap();
                                assert_eq!(text, "sensitive payload");
                                assert_eq!(trust, Some(outer));
                            }
                        }
                    }
                }
            }
        }
    });
}

#[test]
fn prompt_text_projection_moves_nested_unique_prompt_capacity() {
    super::tests::with_prompt_eval(|eval, span| {
        let mut text = String::with_capacity(128);
        text.push_str("first");
        let pointer = text.as_ptr();
        let prompt = PromptValue::new(vec![
            PromptMessage {
                role: PromptRole::User,
                text: text.into(),
                trust: None,
            },
            PromptMessage {
                role: PromptRole::Assistant,
                text: "second".into(),
                trust: None,
            },
        ]);
        let input = wrapped(
            TrustWrapper::Trusted,
            message(InterpValue::Prompt(prompt), String::new()),
        );
        let ((text, trust), cost) = measure(|| {
            eval.prompt_channel_content("system", input, span, false)
                .unwrap()
        });
        assert_eq!(text, "first\nsecond");
        assert_eq!(text.as_ptr(), pointer);
        assert_eq!(trust, Some(TrustWrapper::Trusted));
        assert_eq!(cost.count, 0, "{cost:?}");
    });
}

#[test]
fn unsupported_prompt_text_does_not_format_rejected_payloads() {
    super::tests::with_prompt_eval(|eval, span| {
        let secret = wrapped(
            TrustWrapper::Secret,
            InterpValue::String("must-not-leak".into()),
        );
        for input in [
            InterpValue::Array(vec![secret.clone()].into()),
            InterpValue::Nominal {
                ty: etas_types::TypeId(0),
                value: secret.into(),
            },
        ] {
            let error = eval
                .prompt_channel_content("user", input, span, true)
                .unwrap_err();
            assert_eq!(error.code, AnalysisDiagnosticCode::InvalidArguments);
            assert!(!error.message.contains("must-not-leak"));
        }
    });
}

#[test]
fn prompt_data_rejects_a_deep_secret_before_serialization() {
    super::tests::with_prompt_eval(|eval, span| {
        let mut input = wrapped(
            TrustWrapper::Secret,
            InterpValue::String("must-not-leak".into()),
        );
        for level in 0..30_000 {
            input = if level % 2 == 0 {
                InterpValue::OptionSome(input.into())
            } else {
                InterpValue::Array(vec![input].into())
            };
        }
        let error = eval
            .prompt_channel_content("data", input, span, true)
            .unwrap_err();
        assert_eq!(error.code, AnalysisDiagnosticCode::InvalidArguments);
        assert!(error.message.contains("cannot encode secret"));
        assert!(!error.message.contains("must-not-leak"));
    });
}

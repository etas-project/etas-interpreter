use super::*;
use crate::{api::RunOptions, testing::allocation::measure};

pub(super) fn with_prompt_eval(test: impl FnOnce(&EvalContext<'_>, Span)) {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> Prompt { return Prompt.new().data([\"x\"]); }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let span = checked
        .hir
        .exprs
        .iter()
        .find_map(|(_, expr)| match expr {
            HirExpr::MethodCall { span, .. } => Some(*span),
            _ => None,
        })
        .unwrap();
    let options = RunOptions::default();
    let (program, mut run_state) = EvalContextInput {
        storage_limits: Default::default(),
        code_definitions: None,
        syntax_limits: Default::default(),
        event_observer: None,
        execution: etas_host::execution::ExecutionScope::new_owned(),
        checked: &checked,
        plan: &plan,
        host_context: options.host_context,
        model_policy: options.model_policy,
        execution_limits: options.execution_limits,
        consumed_steps: 0,
        current_session: None,
        entry_item: checked.entry.unwrap(),
        entry_args: &[],
    }
    .into_parts();
    let eval = EvalContext::new(program, &mut run_state);
    test(&eval, span);
}

#[test]
fn prompt_data_cold_text_preserves_aliases_without_a_retained_input_cache() {
    use etas_builtin::text::input::TextInput;
    with_prompt_eval(|eval, span| {
        for count in [1000, 2000, 4000] {
            let prefix = "中😀\n\"\\\u{0}payload".repeat(count);
            let source = crate::value::StringValue::from(prefix.as_str());
            let alias = source.clone();
            let source = source.concat("tail".into()).unwrap();
            drop(alias);
            assert!(source.contiguous().is_none());
            let value = InterpValue::Record(
                vec![(
                    "payload".into(),
                    InterpValue::Array(vec![InterpValue::String(source.clone())].into()),
                )]
                .into(),
            );
            let expected = serde_json::json!({"payload": [format!("{prefix}tail")]}).to_string();
            let ((text, trust), cost) = measure(|| {
                eval.prompt_channel_content("data", value.clone(), span, true)
                    .unwrap()
            });
            assert_eq!(text, expected);
            assert_eq!(trust, None);
            assert!(
                source.contiguous().is_none(),
                "n={count}: source was cached"
            );
            assert_eq!(source.len(), prefix.len() + 4);
            eprintln!(
                "cold Prompt.data n={count}, output={}: {cost:?}",
                text.len()
            );
        }
    });
}

#[test]
fn prompt_data_streams_borrowed_payloads_and_rejects_nested_secrets() {
    with_prompt_eval(|eval, span| {
        for count in [1000, 2000, 4000] {
            let source = "text".repeat(count * 256);
            let pointer = source.as_ptr();
            let prompt = crate::value::PromptValue::new(vec![crate::value::PromptMessage {
                role: crate::value::PromptRole::Assistant,
                text: source.into(),
                trust: Some(etas_types::TrustWrapper::Trusted),
            }]);
            let saved =
                crate::orchestration::ValueSnapshot::capture(&InterpValue::Prompt(prompt.clone()))
                    .unwrap();
            for method in ["user", "assistant", "system"] {
                let ((text, trust), cost) = measure(|| {
                    eval.prompt_channel_content(
                        method,
                        InterpValue::Prompt(prompt.clone()),
                        span,
                        true,
                    )
                    .unwrap()
                });
                assert_eq!(text.as_ptr(), pointer);
                assert_eq!(text.len(), count * 1024);
                assert_eq!(trust, None);
                assert_eq!(cost.count, 0, "{method} n={count}: {cost:?}");
                assert_eq!(cost.bytes, 0, "{method} n={count}: {cost:?}");
            }
            let error = eval
                .prompt_channel_content("system", InterpValue::Prompt(prompt.clone()), span, false)
                .unwrap_err();
            assert_eq!(error.code, AnalysisDiagnosticCode::InvalidArguments);
            assert!(error.message.contains("requires Trusted"));
            assert_eq!(
                saved.restore().unwrap(),
                InterpValue::Prompt(prompt),
                "channel projection must not change role, trust or checkpoint content"
            );
        }
        for count in [1000, 2000, 4000] {
            let value = InterpValue::Record(
                vec![(
                    "data".into(),
                    InterpValue::List(
                        (0..count)
                            .map(|_| InterpValue::String("payload".repeat(128).into()))
                            .collect::<Vec<_>>()
                            .into(),
                    ),
                )]
                .into(),
            );
            let (reference, old) = measure(|| {
                let host = crate::eval::host_value::interp_to_host_value(&value).unwrap();
                etas_host::host_value_to_json_string(&host).unwrap()
            });
            let ((text, trust), cost) = measure(|| {
                eval.prompt_channel_content("data", value.clone(), span, true)
                    .unwrap()
            });
            assert_eq!(text, reference);
            assert_eq!(trust, None);
            assert!(
                cost.count < 32,
                "only output growth and borrowed field index: {cost:?}"
            );
            assert!(old.count >= count, "reference materializes the payload");
            assert!(cost.bytes < old.bytes);
            eprintln!(
                "Prompt.data n={count}, output={} bytes, materialized={old:?}, borrowed={cost:?}",
                text.len()
            );
        }

        let secret = InterpValue::Trust {
            wrapper: etas_types::TrustWrapper::Secret,
            value: crate::value::SharedValue::new(InterpValue::String(
                "sensitive payload".repeat(128).into(),
            )),
        };
        let message = crate::value::MessageValue {
            id: "m".into(),
            from: None,
            to: None,
            role: crate::value::MessageRoleValue::User,
            session: None,
            created_at: "now".into(),
            payload: secret.clone().into(),
            provenance: None,
        };
        for value in [
            InterpValue::Nominal {
                ty: etas_types::TypeId(0),
                value: crate::value::SharedValue::new(secret.clone()),
            },
            InterpValue::Array(vec![secret].into()),
            InterpValue::Message(message.clone()),
            InterpValue::Conversation(crate::value::ConversationValue {
                selected_context: None,
                session: "s".into(),
                history_fence: None,
                messages: vec![message].into(),
                cursor: None,
            }),
        ] {
            let error = eval
                .prompt_channel_content("data", value, span, true)
                .unwrap_err();
            assert_eq!(error.code, AnalysisDiagnosticCode::InvalidArguments);
            assert!(error.message.contains("cannot encode secret values"));
            assert!(!error.message.contains("sensitive payload"));
        }
    });
}

#[test]
fn prompt_data_deep_wrappers_keep_json_and_secret_rejection_without_recursive_projection() {
    if crate::testing::in_subprocess("ETAS_TEST_PROMPT_ABI_WRAPPERS") {
        return;
    }
    with_prompt_eval(|eval, span| {
        let marker = "SECRET_MARKER".repeat(1024);
        let scalar = InterpValue::String(marker.clone().into());
        let ((reference, _), flat_cost) = measure(|| {
            eval.prompt_channel_content("data", scalar.clone(), span, true)
                .unwrap()
        });
        for depth in [1000, 2000, 4000, 30_000] {
            for secret in [false, true] {
                let mut value = if secret {
                    InterpValue::Trust {
                        wrapper: etas_types::TrustWrapper::Secret,
                        value: scalar.clone().into(),
                    }
                } else {
                    scalar.clone()
                };
                for layer in 0..depth {
                    value = if layer % 2 == 0 {
                        InterpValue::Nominal {
                            ty: etas_types::TypeId(layer),
                            value: value.into(),
                        }
                    } else {
                        InterpValue::Trust {
                            wrapper: etas_types::TrustWrapper::Public,
                            value: value.into(),
                        }
                    };
                }
                let (result, cost) =
                    measure(|| eval.prompt_channel_content("data", value.clone(), span, true));
                if secret {
                    let error = result.unwrap_err();
                    assert_eq!(error.code, AnalysisDiagnosticCode::InvalidArguments);
                    assert!(error.message.contains("cannot encode secret values"));
                    assert!(!error.message.contains("SECRET_MARKER"));
                    assert!(cost.bytes < 4096, "depth={depth}: {cost:?}");
                } else {
                    let (text, trust) = result.unwrap();
                    assert_eq!(text, reference);
                    assert_eq!(trust, None);
                    assert!(
                        cost.count <= flat_cost.count + 16,
                        "depth={depth}: {cost:?}"
                    );
                    assert!(
                        cost.bytes <= flat_cost.bytes + 4096,
                        "depth={depth}: {cost:?}"
                    );
                }
                eprintln!(
                    "Prompt.data wrapper depth={depth}, secret={secret}: {cost:?}; flat={flat_cost:?}"
                );
            }
        }
    });
}

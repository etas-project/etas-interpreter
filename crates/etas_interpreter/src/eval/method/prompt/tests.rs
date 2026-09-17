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
    let eval = EvalContext::new(EvalContextInput {
        storage_limits: Default::default(),
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
    });
    test(&eval, span);
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

use super::*;
use crate::intrinsic::dispatch::{AgentSupportCallable, CheckedStdIntrinsicCall};

impl EvalContext<'_> {
    pub(super) fn execute_agent_support_callable(
        &mut self,
        kind: AgentSupportCallable,
        checked: &CheckedStdIntrinsicCall,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        use AgentSupportCallable as S;
        match kind {
            S::PromptNew => ControlSignal::Value(InterpValue::Prompt(Default::default())),
            S::PromptSystem | S::PromptUser | S::PromptAssistant | S::PromptData => {
                let [InterpValue::Prompt(messages), value] = args.as_slice() else {
                    return ControlSignal::missing_checked_fact(
                        "prompt call violates its checked receiver ABI",
                        span,
                    );
                };
                let (name, role) = match kind {
                    S::PromptSystem => ("system", crate::value::PromptRole::System),
                    S::PromptUser => ("user", crate::value::PromptRole::User),
                    S::PromptAssistant => ("assistant", crate::value::PromptRole::Assistant),
                    S::PromptData => ("data", crate::value::PromptRole::Data),
                    _ => unreachable!(),
                };
                let content = match self.checked.hir.exprs.get(checked.origin) {
                    Some(HirExpr::MethodCall { args, .. }) => args.first(),
                    Some(HirExpr::Call { args, .. }) => args.get(1),
                    _ => None,
                };
                let literal = content.is_some_and(|arg| {
                    let (HirArg::Positional(expr) | HirArg::Named { value: expr, .. }) = arg;
                    matches!(
                        self.checked.hir.exprs.get(*expr),
                        Some(HirExpr::Literal(HirLiteral::String { .. }))
                    )
                });
                self.finish_prompt_value_method_arg(
                    messages.clone(),
                    name,
                    role,
                    kind != S::PromptSystem || literal,
                    value.clone(),
                    span,
                )
            }
            S::MessageNew => self.eval_message_type_method_values("new", &args, span),
            S::MessageWithSession => {
                self.eval_message_type_method_values("with_session", &args, span)
            }
            S::MessageCast => {
                let [InterpValue::Message(message)] = args.as_slice() else {
                    return ControlSignal::missing_checked_fact(
                        "message cast violates its checked receiver ABI",
                        span,
                    );
                };
                let Some(etas_types::Type::Option(message_type)) =
                    self.checked.type_store.get(checked.result_type)
                else {
                    return ControlSignal::missing_checked_fact(
                        "message cast has no checked Option result",
                        span,
                    );
                };
                let Some(etas_types::Type::Message(payload)) =
                    self.checked.type_store.get(*message_type)
                else {
                    return ControlSignal::missing_checked_fact(
                        "message cast has no checked payload type",
                        span,
                    );
                };
                self.cast_message_payload(message.clone(), *payload, span)
            }
            S::SessionContinueOrNew => self.eval_session_config_type_method_values(
                checked.origin,
                "continue_or_new",
                &args,
                span,
            ),
            S::ConversationLoad => {
                self.eval_conversation_type_method_values(checked.origin, "load", &args, span)
            }
            S::MemorySelectionLimit => {
                let mut args = args.into_iter();
                let (Some(mut selection), Some(value), None) =
                    (args.next(), args.next(), args.next())
                else {
                    return ControlSignal::missing_checked_fact(
                        "memory selection limit violates its checked ABI",
                        span,
                    );
                };
                let count = match self.decode_memory_selection_limit(&value, span) {
                    Ok(count) => count,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                };
                let InterpValue::MemorySelection { limit, .. } = &mut selection else {
                    return ControlSignal::missing_checked_fact(
                        "memory selection limit has no checked selection receiver",
                        span,
                    );
                };
                *limit = Some(count);
                ControlSignal::Value(selection)
            }
        }
    }
}

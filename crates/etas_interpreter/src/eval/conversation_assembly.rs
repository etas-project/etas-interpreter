use etas_core::Span;
use etas_host::HostValue;

use crate::{
    control::ControlSignal,
    intrinsic::dispatch::{AgentSupportCallable as Operation, CheckedStdIntrinsicCall},
    value::{
        InterpValue as V,
        conversation::{self, protocol},
    },
};

impl super::EvalContext<'_> {
    pub(super) fn execute_conversation_assembly(
        &self,
        operation: Operation,
        checked: &CheckedStdIntrinsicCall,
        args: Vec<V>,
        span: Span,
    ) -> ControlSignal {
        let mut args = args.into_iter();
        if operation == Operation::ConversationFromPrompt {
            let (Some(V::Prompt(prompt)), None) = (args.next(), args.next()) else {
                return ControlSignal::missing_checked_fact(
                    "from_prompt requires its checked Prompt ABI",
                    span,
                );
            };
            return match conversation::from_prompt(prompt, &self.storage_limits) {
                Ok(value) => ControlSignal::Value(V::Conversation(value)),
                Err(error) => ControlSignal::runtime_fault(error.message, span),
            };
        }
        let (Some(V::Conversation(value)), Some(payload), None) =
            (args.next(), args.next(), args.next())
        else {
            return ControlSignal::missing_checked_fact(
                "conversation assembly violates its checked argument ABI",
                span,
            );
        };
        let result = match operation {
            Operation::ConversationAppendStep => {
                conversation::append_step(value, &payload, &self.storage_limits)
            }
            Operation::ConversationAppendResults => match payload {
                V::Array(results) => {
                    conversation::append_results(value, &results, &self.storage_limits)
                }
                _ => Err(protocol::ProtocolError {
                    code: "invalid_protocol",
                    message: "tool results are not an Array".into(),
                }),
            },
            _ => unreachable!(),
        };
        match result {
            Ok(value) => ControlSignal::Value(V::Variant {
                name: "Ok".into(),
                fields: vec![V::Conversation(value)].into(),
            }),
            Err(error) => self.conversation_protocol_error(checked.result_type, error, span),
        }
    }

    fn conversation_protocol_error(
        &self,
        result: etas_types::TypeId,
        error: protocol::ProtocolError,
        span: Span,
    ) -> ControlSignal {
        let Some(etas_types::Type::Result { err, .. }) = self.checked.type_store.get(result) else {
            return ControlSignal::missing_checked_fact(
                "conversation assembly has no checked Result ABI",
                span,
            );
        };
        let data = HostValue::Record(vec![
            ("code".into(), HostValue::String(error.code.into())),
            ("message".into(), HostValue::String(error.message)),
            (
                "path".into(),
                HostValue::Variant {
                    name: "None".into(),
                    fields: Vec::new(),
                },
            ),
        ]);
        match super::host_value::decode_checked_host_value(data, *err, self.checked) {
            Ok(error) => ControlSignal::Value(V::Variant {
                name: "Err".into(),
                fields: vec![error].into(),
            }),
            Err(error) => ControlSignal::missing_checked_fact(error, span),
        }
    }
}

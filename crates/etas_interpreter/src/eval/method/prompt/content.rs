use super::*;
use crate::value::StringValue;
use etas_types::TrustWrapper;

pub(super) fn text(
    mut value: InterpValue,
    method: &str,
    span: Span,
    allow_plain_system_content: bool,
) -> Result<(StringValue, Option<TrustWrapper>), ExecutionFault> {
    let mut state = Projection {
        method,
        span,
        allow_plain_system_content,
        outer_trust: None,
    };
    // Move unique payloads so owned Prompt text keeps its reusable capacity.
    // At the first alias, retain that owner and borrow the rest of the chain.
    let shared = loop {
        let payload = match value {
            InterpValue::Trust { wrapper, value } => {
                state.enter(wrapper)?;
                value
            }
            InterpValue::Message(message) => message.payload,
            InterpValue::String(text) => return state.finish(|| text),
            InterpValue::Prompt(parts) => return state.finish(|| parts.into_text()),
            other => return Err(state.unsupported(&other)),
        };
        match payload.try_into_value() {
            Ok(next) => value = next,
            Err(shared) => break shared,
        }
    };
    let mut value = shared.as_ref();
    loop {
        value = match value {
            InterpValue::Trust { wrapper, value } => {
                state.enter(*wrapper)?;
                value
            }
            InterpValue::Message(message) => &message.payload,
            InterpValue::String(text) => return state.finish(|| text.clone()),
            InterpValue::Prompt(parts) => return state.finish(|| parts.clone().into_text()),
            other => return Err(state.unsupported(other)),
        };
    }
}

struct Projection<'a> {
    method: &'a str,
    span: Span,
    allow_plain_system_content: bool,
    outer_trust: Option<TrustWrapper>,
}

impl Projection<'_> {
    fn enter(&mut self, wrapper: TrustWrapper) -> Result<(), ExecutionFault> {
        if wrapper == TrustWrapper::Secret {
            return Err(self.error("Secret[T] values are not prompt-encodable by default"));
        }
        if self.method == "system" && wrapper != TrustWrapper::Trusted {
            return Err(self.untrusted_system());
        }
        self.allow_plain_system_content |= wrapper == TrustWrapper::Trusted;
        self.outer_trust.get_or_insert(wrapper);
        Ok(())
    }

    fn finish(
        self,
        text: impl FnOnce() -> StringValue,
    ) -> Result<(StringValue, Option<TrustWrapper>), ExecutionFault> {
        if self.method == "system" && !self.allow_plain_system_content {
            return Err(self.untrusted_system());
        }
        Ok((text(), self.outer_trust))
    }

    fn unsupported(&self, value: &InterpValue) -> ExecutionFault {
        self.error(format!(
            "Prompt.{} expects a string-compatible argument, got {}",
            self.method,
            prompt_data_kind(value)
        ))
    }

    fn untrusted_system(&self) -> ExecutionFault {
        self.error("Prompt.system requires Trusted[T] content or a checked static string literal")
    }

    fn error(&self, message: impl Into<String>) -> ExecutionFault {
        ExecutionFault::new(AnalysisDiagnosticCode::InvalidArguments, self.span, message)
    }
}

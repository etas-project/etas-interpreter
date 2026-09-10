use super::storage::{field, invalid, text};
use super::*;
use etas_host::session::{SessionContextContent, SessionHistoryFence};
use etas_host::{HostError, HostErrorCode};

impl EvalContext<'_> {
    pub(super) fn checked_session_config(
        &self,
        value: &InterpValue,
        expected: Option<&etas_types::TypeId>,
    ) -> Result<etas_host::SessionConfig, HostError> {
        let ty =
            super::resolve_std_type(self.checked, &["std", "agent", "session", "SessionConfig"])
                .ok_or_else(|| invalid("missing checked SessionConfig type"))?;
        if expected != Some(&ty)
            || (!matches!(value, InterpValue::Nominal {ty: actual,..} if *actual == ty)
                && !matches!(value, InterpValue::Variant {name,fields} if name == "SessionConfig.continue_or_new" && fields.len() == 1))
        {
            return Err(invalid("config does not match checked SessionConfig ABI"));
        }
        let config = super::method::helpers::session_config_from_value(value)
            .ok_or_else(|| invalid("invalid SessionConfig value"))?;
        let config = super::boundary_session::session_config_to_host(&config).map_err(invalid)?;
        if config.id.is_empty() || config.id.len() > self.storage_limits.clone().max_value_bytes {
            return Err(invalid("invalid bounded session identity"));
        }
        Ok(config)
    }

    pub(super) fn session_history_fence(
        &self,
        value: &InterpValue,
    ) -> Result<SessionHistoryFence, HostError> {
        let fields = self
            .checked_storage_record(value, &["std", "agent", "session", "SessionHistoryFence"])?;
        if fields.len() != 1 {
            return Err(invalid("invalid fence fields"));
        }
        SessionHistoryFence::from_token(
            text(&fields, "opaque", &self.storage_limits)?.to_owned(),
            &self.storage_limits.clone(),
        )
    }

    pub(super) fn session_context_content(
        &self,
        value: &InterpValue,
    ) -> Result<SessionContextContent, HostError> {
        let fields = self
            .checked_storage_record(value, &["std", "agent", "session", "SessionContextContent"])?;
        if fields.len() != 2 {
            return Err(invalid("invalid context fields"));
        }
        let text = text(&fields, "text", &self.storage_limits)?;
        let InterpValue::Map(provenance) = field(&fields, "provenance")? else {
            return Err(invalid("context provenance must be Map<string,string>"));
        };
        let provenance = provenance.borrow();
        let limits = self.storage_limits.clone();
        if provenance.len() > limits.max_nodes {
            return Err(exceeded());
        }
        let mut size = text.len();
        for entry in provenance.iter() {
            let (key, value) = strings(entry)?;
            size = size
                .checked_add(key.len())
                .and_then(|n| n.checked_add(value.len()))
                .ok_or_else(exceeded)?;
        }
        if size > limits.max_value_bytes {
            return Err(exceeded());
        }
        let mut mapped = std::collections::BTreeMap::new();
        for entry in provenance.iter() {
            let (key, value) = strings(entry)?;
            if mapped.insert(key.to_owned(), value.to_owned()).is_some() {
                return Err(invalid("duplicate context provenance key"));
            }
        }
        Ok(SessionContextContent {
            text: text.into(),
            provenance: mapped,
        })
    }
}
fn strings(entry: &(InterpValue, InterpValue)) -> Result<(&str, &str), HostError> {
    match entry {
        (InterpValue::String(key), InterpValue::String(value)) => Ok((key, value)),
        _ => Err(invalid("invalid context provenance entry")),
    }
}
fn exceeded() -> HostError {
    HostError::new(
        HostErrorCode::BudgetExceeded,
        "context exceeds storage limits",
    )
}

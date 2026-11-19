use super::storage::{field, invalid, text};
use super::*;
use etas_host::session::{SessionContextContent, SessionHistoryFence};
use etas_host::{HostError, HostErrorCode};
use etas_std::RuntimeTypeRole as Role;

impl EvalContext<'_> {
    pub(super) fn checked_session_config(
        &self,
        value: &InterpValue,
        expected: Option<&etas_types::TypeId>,
    ) -> Result<etas_host::SessionConfig, HostError> {
        let ty = super::boundary_session::session_std_type(
            self.checked,
            Role::SessionConfig,
            "SessionConfig",
        )
        .ok_or_else(|| invalid("missing checked SessionConfig type"))?;
        if expected != Some(&ty) {
            return Err(invalid("config does not match checked SessionConfig ABI"));
        }
        let config = self.session_config_parts(value).map_err(invalid)?;
        let config = super::boundary_session::session_config_to_host(&config, self.checked)
            .map_err(invalid)?;
        if config.id.is_empty() || config.id.len() > self.storage_limits.clone().max_value_bytes {
            return Err(invalid("invalid bounded session identity"));
        }
        Ok(config)
    }

    pub(super) fn session_config_parts(
        &self,
        value: &InterpValue,
    ) -> Result<crate::value::SessionConfigValue, String> {
        let config = super::boundary_session::session_std_type(
            self.checked,
            Role::SessionConfig,
            "SessionConfig",
        )
        .ok_or("missing checked SessionConfig identity")?;
        crate::value::contract::runtime::validate_value(
            self.checked,
            self.plan,
            value,
            config,
            &self.storage_limits,
        )?;
        let InterpValue::Nominal { value, .. } = value else {
            return Err("SessionConfig is missing its checked identity".into());
        };
        let InterpValue::Record(fields) = value.as_ref() else {
            return Err("SessionConfig has no record representation".into());
        };
        let fields = fields.borrow();
        let get = |name: &str| {
            fields
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, value)| value)
                .ok_or_else(|| format!("SessionConfig is missing {name}"))
        };
        let InterpValue::String(id) = super::std_call::nominal_representation_ref(get("id")?)
        else {
            return Err("SessionId has no string representation".into());
        };
        Ok(crate::value::SessionConfigValue {
            id: id.to_string(),
            context: Some(Box::new(get("context")?.clone())),
            retention: Some(Box::new(get("retention")?.clone())),
        })
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
            text(fields, "opaque", &self.storage_limits)?.to_owned(),
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
        let text = text(fields, "text", &self.storage_limits)?;
        let limits = self.storage_limits.clone();
        let HostValue::Map(provenance) = super::host_value::checked_map_to_host(
            field(fields, "provenance")?,
            self.checked,
            &limits,
        )
        .map_err(invalid)?
        else {
            return Err(invalid("context provenance must be Map<string,string>"));
        };
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
fn strings(entry: &(HostValue, HostValue)) -> Result<(&str, &str), HostError> {
    match entry {
        (HostValue::String(key), HostValue::String(value)) => Ok((key, value)),
        _ => Err(invalid("invalid context provenance entry")),
    }
}
fn exceeded() -> HostError {
    HostError::new(
        HostErrorCode::BudgetExceeded,
        "context exceeds storage limits",
    )
}

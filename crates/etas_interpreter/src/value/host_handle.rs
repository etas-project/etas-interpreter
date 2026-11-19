use std::fmt;

use etas_host::{ByteStreamRef, SecretRef, SecretValue, TcpStreamRef, TlsStreamRef};
use etas_types::TypeId;

use super::InterpValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostHandleKind {
    TcpStream,
    TlsStream,
    SecretValue,
    BrowserSession,
    CommandProcess,
}

#[derive(Clone, PartialEq, Eq)]
pub struct HostHandleValue {
    nominal_type: etas_types::TypeId,
    payload: HostHandlePayload,
}

#[derive(Clone, PartialEq, Eq)]
enum HostHandlePayload {
    TcpStream(TcpStreamRef),
    TlsStream(TlsStreamRef),
    SecretValue(SecretRef),
    BrowserSession(String),
    CommandProcess(String),
}

impl HostHandleValue {
    pub(crate) fn tcp_stream(nominal_type: etas_types::TypeId, stream: TcpStreamRef) -> Self {
        Self {
            nominal_type,
            payload: HostHandlePayload::TcpStream(stream),
        }
    }

    pub(crate) fn tls_stream(nominal_type: etas_types::TypeId, stream: TlsStreamRef) -> Self {
        Self {
            nominal_type,
            payload: HostHandlePayload::TlsStream(stream),
        }
    }

    pub(crate) fn secret_value(nominal_type: etas_types::TypeId, secret: SecretValue) -> Self {
        Self {
            nominal_type,
            payload: HostHandlePayload::SecretValue(secret.reference().clone()),
        }
    }

    pub(crate) fn browser_session(nominal_type: etas_types::TypeId, id: String) -> Self {
        Self {
            nominal_type,
            payload: HostHandlePayload::BrowserSession(id),
        }
    }

    pub(crate) fn command_process(nominal_type: etas_types::TypeId, id: String) -> Self {
        Self {
            nominal_type,
            payload: HostHandlePayload::CommandProcess(id),
        }
    }

    pub fn nominal_type(&self) -> etas_types::TypeId {
        self.nominal_type
    }

    // The revision bridge has already proved the nominal type correspondence.
    // Preserve the sealed capability; only its program-local type tag changes.
    pub(crate) fn into_revision_type(mut self, ty: etas_types::TypeId) -> Self {
        self.nominal_type = ty;
        self
    }

    pub(crate) fn kind(&self) -> HostHandleKind {
        match self.payload {
            HostHandlePayload::TcpStream(_) => HostHandleKind::TcpStream,
            HostHandlePayload::TlsStream(_) => HostHandleKind::TlsStream,
            HostHandlePayload::SecretValue(_) => HostHandleKind::SecretValue,
            HostHandlePayload::BrowserSession(_) => HostHandleKind::BrowserSession,
            HostHandlePayload::CommandProcess(_) => HostHandleKind::CommandProcess,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind() {
            HostHandleKind::TcpStream => "tcp_stream",
            HostHandleKind::TlsStream => "tls_stream",
            HostHandleKind::SecretValue => "secret_value",
            HostHandleKind::BrowserSession => "browser_session",
            HostHandleKind::CommandProcess => "command_process",
        }
    }

    pub(crate) fn byte_stream_ref(&self) -> Option<ByteStreamRef> {
        match &self.payload {
            HostHandlePayload::TcpStream(stream) => Some(stream.as_byte_stream()),
            HostHandlePayload::TlsStream(stream) => Some(stream.as_byte_stream()),
            HostHandlePayload::SecretValue(_)
            | HostHandlePayload::BrowserSession(_)
            | HostHandlePayload::CommandProcess(_) => None,
        }
    }

    pub(crate) fn tcp_stream_ref(&self) -> Option<TcpStreamRef> {
        match &self.payload {
            HostHandlePayload::TcpStream(stream) => Some(stream.clone()),
            HostHandlePayload::TlsStream(_)
            | HostHandlePayload::SecretValue(_)
            | HostHandlePayload::BrowserSession(_)
            | HostHandlePayload::CommandProcess(_) => None,
        }
    }

    pub(crate) fn secret_ref(&self) -> Option<SecretRef> {
        match &self.payload {
            HostHandlePayload::SecretValue(reference) => Some(reference.clone()),
            HostHandlePayload::TcpStream(_)
            | HostHandlePayload::TlsStream(_)
            | HostHandlePayload::BrowserSession(_)
            | HostHandlePayload::CommandProcess(_) => None,
        }
    }

    pub(crate) fn browser_session_id(&self) -> Option<&str> {
        match &self.payload {
            HostHandlePayload::BrowserSession(id) => Some(id),
            HostHandlePayload::TcpStream(_)
            | HostHandlePayload::TlsStream(_)
            | HostHandlePayload::SecretValue(_)
            | HostHandlePayload::CommandProcess(_) => None,
        }
    }

    pub(crate) fn command_process_id(&self) -> Option<&str> {
        match &self.payload {
            HostHandlePayload::CommandProcess(id) => Some(id),
            HostHandlePayload::TcpStream(_)
            | HostHandlePayload::TlsStream(_)
            | HostHandlePayload::SecretValue(_)
            | HostHandlePayload::BrowserSession(_) => None,
        }
    }
}

impl fmt::Debug for HostHandleValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostHandleValue")
            .field("kind", &self.kind())
            .field("nominal_type", &self.nominal_type)
            .finish_non_exhaustive()
    }
}

pub(crate) fn source_handle_type(
    checked: &etas_frontend::CheckedProject,
    wrapper: TypeId,
    field: &str,
) -> Result<TypeId, String> {
    etas_types::record_fields_with_applied_params(&checked.type_store, wrapper)
        .map_err(|error| error.to_string())?
        .and_then(|record| record.fields.into_iter().find(|item| item.name == field))
        .filter(|item| {
            checked
                .types
                .known_std_types
                .opaque_values
                .contains(&item.ty)
        })
        .map(|item| item.ty)
        .ok_or_else(|| format!("source capability has no checked opaque `{field}` field"))
}

pub(crate) fn source_handle(
    checked: &etas_frontend::CheckedProject,
    wrapper: TypeId,
    field: &str,
    create: impl FnOnce(TypeId) -> HostHandleValue,
) -> Result<InterpValue, String> {
    let handle = create(source_handle_type(checked, wrapper, field)?);
    Ok(InterpValue::Nominal {
        ty: wrapper,
        value: InterpValue::Record(vec![(field.into(), InterpValue::HostHandle(handle))].into())
            .into(),
    })
}

pub(crate) fn open_source_handle<'a>(
    checked: &etas_frontend::CheckedProject,
    wrapper: TypeId,
    field: &str,
    value: &'a InterpValue,
) -> Result<&'a InterpValue, String> {
    let slot = source_handle_type(checked, wrapper, field)?;
    if let InterpValue::Nominal { ty, value } = value
        && *ty == wrapper
        && let InterpValue::Record(record) = value.as_ref()
        && let Some(value @ InterpValue::HostHandle(handle)) = record.get_ref(field)
        && handle.nominal_type() == slot
    {
        return Ok(value);
    }
    Err(format!(
        "source capability requires a sealed `{field}` handle"
    ))
}

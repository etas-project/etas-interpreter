use std::{cell::RefCell, fmt, rc::Rc};

use etas_host::{ByteStreamRef, FileStreamRef, SecretRef, SecretValue, TcpStreamRef, TlsStreamRef};
use etas_types::TypeId;

use super::InterpValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostHandleKind {
    FileStream,
    TcpStream,
    TlsStream,
    SecretValue,
    BrowserSession,
    CommandProcess,
    Pty,
    ClockDomain,
    ReadOwner,
    SessionLease,
}

#[derive(Clone, PartialEq, Eq)]
pub struct HostHandleValue {
    nominal_type: etas_types::TypeId,
    payload: HostHandlePayload,
}

#[derive(Clone, PartialEq, Eq)]
enum HostHandlePayload {
    FileStream(FileStreamRef),
    TcpStream(TcpStreamRef),
    TlsStream(TlsStreamRef),
    SecretValue(SecretRef),
    BrowserSession(String),
    CommandProcess(String),
    Pty {
        id: String,
        selector: Option<Vec<String>>,
    },
    ClockDomain(String),
    ReadOwner(etas_host::StreamReadScope),
    SessionLease(LeaseHandle),
}

#[derive(Clone)]
struct LeaseHandle(Rc<RefCell<LeaseState>>);

struct LeaseState {
    current: Option<etas_host::session::SessionLease>,
    pending_delivery: Option<(etas_host::HostRequestId, Rc<()>)>,
}

impl PartialEq for LeaseHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for LeaseHandle {}

impl HostHandleValue {
    pub(crate) fn file_stream(nominal_type: TypeId, stream: FileStreamRef) -> Self {
        Self {
            nominal_type,
            payload: HostHandlePayload::FileStream(stream),
        }
    }

    pub(crate) fn file_stream_ref(&self) -> Option<FileStreamRef> {
        match &self.payload {
            HostHandlePayload::FileStream(stream) => Some(stream.clone()),
            _ => None,
        }
    }

    pub(crate) fn session_lease(
        nominal_type: TypeId,
        lease: etas_host::session::SessionLease,
        pending_delivery: Option<(etas_host::HostRequestId, Rc<()>)>,
    ) -> Self {
        Self {
            nominal_type,
            payload: HostHandlePayload::SessionLease(LeaseHandle(Rc::new(RefCell::new(
                LeaseState {
                    current: Some(lease),
                    pending_delivery,
                },
            )))),
        }
    }

    pub(crate) fn take_handoff_delivery(&self) -> Option<etas_host::HostRequestId> {
        let HostHandlePayload::SessionLease(handle) = &self.payload else {
            return None;
        };
        handle
            .0
            .borrow_mut()
            .pending_delivery
            .take()
            .map(|(request, _)| request)
    }

    pub(crate) fn take_session_lease(&self) -> Option<etas_host::session::SessionLease> {
        match &self.payload {
            HostHandlePayload::SessionLease(handle) => {
                let mut state = handle.0.borrow_mut();
                state.pending_delivery.take();
                state.current.take()
            }
            _ => None,
        }
    }

    pub(crate) fn live_session_lease(&self) -> bool {
        matches!(&self.payload, HostHandlePayload::SessionLease(handle) if handle.0.borrow().current.is_some())
    }

    pub(crate) fn read_owner(nominal_type: TypeId, scope: etas_host::StreamReadScope) -> Self {
        Self {
            nominal_type,
            payload: HostHandlePayload::ReadOwner(scope),
        }
    }

    pub(crate) fn with_read_scope(&self, scope: &etas_host::StreamReadScope) -> Option<Self> {
        let payload = match &self.payload {
            HostHandlePayload::FileStream(stream) => {
                HostHandlePayload::FileStream(stream.with_read_scope(scope))
            }
            HostHandlePayload::TcpStream(stream) => {
                HostHandlePayload::TcpStream(stream.with_read_scope(scope))
            }
            HostHandlePayload::TlsStream(stream) => {
                HostHandlePayload::TlsStream(stream.with_read_scope(scope))
            }
            _ => return None,
        };
        Some(Self {
            nominal_type: self.nominal_type,
            payload,
        })
    }
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

    pub(crate) fn pty(nominal_type: TypeId, id: String, selector: Option<Vec<String>>) -> Self {
        Self {
            nominal_type,
            payload: HostHandlePayload::Pty { id, selector },
        }
    }

    pub(crate) fn pty_session(&self) -> Option<(&str, Option<&[String]>)> {
        match &self.payload {
            HostHandlePayload::Pty { id, selector } => Some((id, selector.as_deref())),
            _ => None,
        }
    }

    pub(crate) fn clock_domain(nominal_type: TypeId, domain: String) -> Self {
        Self {
            nominal_type,
            payload: HostHandlePayload::ClockDomain(domain),
        }
    }

    pub(crate) fn clock_domain_id(&self) -> Option<&str> {
        match &self.payload {
            HostHandlePayload::ClockDomain(domain) => Some(domain),
            _ => None,
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
            HostHandlePayload::FileStream(_) => HostHandleKind::FileStream,
            HostHandlePayload::TcpStream(_) => HostHandleKind::TcpStream,
            HostHandlePayload::TlsStream(_) => HostHandleKind::TlsStream,
            HostHandlePayload::SecretValue(_) => HostHandleKind::SecretValue,
            HostHandlePayload::BrowserSession(_) => HostHandleKind::BrowserSession,
            HostHandlePayload::CommandProcess(_) => HostHandleKind::CommandProcess,
            HostHandlePayload::Pty { .. } => HostHandleKind::Pty,
            HostHandlePayload::ClockDomain(_) => HostHandleKind::ClockDomain,
            HostHandlePayload::ReadOwner(_) => HostHandleKind::ReadOwner,
            HostHandlePayload::SessionLease(_) => HostHandleKind::SessionLease,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind() {
            HostHandleKind::FileStream => "file_stream",
            HostHandleKind::TcpStream => "tcp_stream",
            HostHandleKind::TlsStream => "tls_stream",
            HostHandleKind::SecretValue => "secret_value",
            HostHandleKind::BrowserSession => "browser_session",
            HostHandleKind::CommandProcess => "command_process",
            HostHandleKind::Pty => "pty",
            HostHandleKind::ClockDomain => "clock_domain",
            HostHandleKind::ReadOwner => "read_owner",
            HostHandleKind::SessionLease => "session_lease",
        }
    }

    pub(crate) fn byte_stream_ref(&self) -> Option<ByteStreamRef> {
        match &self.payload {
            HostHandlePayload::FileStream(stream) => Some(stream.as_byte_stream()),
            HostHandlePayload::TcpStream(stream) => Some(stream.as_byte_stream()),
            HostHandlePayload::TlsStream(stream) => Some(stream.as_byte_stream()),
            HostHandlePayload::SecretValue(_)
            | HostHandlePayload::BrowserSession(_)
            | HostHandlePayload::CommandProcess(_)
            | HostHandlePayload::Pty { .. }
            | HostHandlePayload::ClockDomain(_)
            | HostHandlePayload::ReadOwner(_)
            | HostHandlePayload::SessionLease(_) => None,
        }
    }

    pub(crate) fn tcp_stream_ref(&self) -> Option<TcpStreamRef> {
        match &self.payload {
            HostHandlePayload::TcpStream(stream) => Some(stream.clone()),
            HostHandlePayload::FileStream(_)
            | HostHandlePayload::TlsStream(_)
            | HostHandlePayload::SecretValue(_)
            | HostHandlePayload::BrowserSession(_)
            | HostHandlePayload::CommandProcess(_)
            | HostHandlePayload::Pty { .. }
            | HostHandlePayload::ClockDomain(_)
            | HostHandlePayload::ReadOwner(_)
            | HostHandlePayload::SessionLease(_) => None,
        }
    }

    pub(crate) fn secret_ref(&self) -> Option<SecretRef> {
        match &self.payload {
            HostHandlePayload::SecretValue(reference) => Some(reference.clone()),
            HostHandlePayload::FileStream(_)
            | HostHandlePayload::TcpStream(_)
            | HostHandlePayload::TlsStream(_)
            | HostHandlePayload::BrowserSession(_)
            | HostHandlePayload::CommandProcess(_)
            | HostHandlePayload::Pty { .. }
            | HostHandlePayload::ClockDomain(_)
            | HostHandlePayload::ReadOwner(_)
            | HostHandlePayload::SessionLease(_) => None,
        }
    }

    pub(crate) fn browser_session_id(&self) -> Option<&str> {
        match &self.payload {
            HostHandlePayload::BrowserSession(id) => Some(id),
            HostHandlePayload::FileStream(_)
            | HostHandlePayload::TcpStream(_)
            | HostHandlePayload::TlsStream(_)
            | HostHandlePayload::SecretValue(_)
            | HostHandlePayload::CommandProcess(_)
            | HostHandlePayload::Pty { .. }
            | HostHandlePayload::ClockDomain(_)
            | HostHandlePayload::ReadOwner(_)
            | HostHandlePayload::SessionLease(_) => None,
        }
    }

    pub(crate) fn command_process_id(&self) -> Option<&str> {
        match &self.payload {
            HostHandlePayload::CommandProcess(id) => Some(id),
            HostHandlePayload::FileStream(_)
            | HostHandlePayload::TcpStream(_)
            | HostHandlePayload::TlsStream(_)
            | HostHandlePayload::SecretValue(_)
            | HostHandlePayload::BrowserSession(_)
            | HostHandlePayload::Pty { .. }
            | HostHandlePayload::ClockDomain(_)
            | HostHandlePayload::ReadOwner(_)
            | HostHandlePayload::SessionLease(_) => None,
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

pub(crate) fn validate_clock_domain(
    checked: &etas_frontend::CheckedProject,
    ty: TypeId,
    domain: &str,
) -> Result<(), String> {
    if !matches!(checked.type_store.get(ty), Some(etas_types::Type::Nominal(owner))
        if owner.name == "std.runtime.time.ClockDomain" && owner.representation.is_none())
        || !checked.types.known_std_types.opaque_values.contains(&ty)
    {
        return Err("clock domain has a different checked opaque identity".into());
    }
    let (process, run) = domain
        .split_once('/')
        .ok_or("invalid clock domain identity")?;
    process
        .parse::<etas_host::replay::ReplayKeyId>()
        .map_err(|error| error.to_string())?;
    run.parse::<etas_host::replay::ReplayKeyId>()
        .map_err(|error| error.to_string())?;
    Ok(())
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

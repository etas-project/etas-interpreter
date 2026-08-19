use std::fmt;

use etas_host::{ByteStreamRef, SecretRef, SecretValue, TcpStreamRef, TlsStreamRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostHandleKind {
    TcpStream,
    TlsStream,
    SecretValue,
    BrowserSession,
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

    pub fn nominal_type(&self) -> etas_types::TypeId {
        self.nominal_type
    }

    pub(crate) fn kind(&self) -> HostHandleKind {
        match self.payload {
            HostHandlePayload::TcpStream(_) => HostHandleKind::TcpStream,
            HostHandlePayload::TlsStream(_) => HostHandleKind::TlsStream,
            HostHandlePayload::SecretValue(_) => HostHandleKind::SecretValue,
            HostHandlePayload::BrowserSession(_) => HostHandleKind::BrowserSession,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind() {
            HostHandleKind::TcpStream => "tcp_stream",
            HostHandleKind::TlsStream => "tls_stream",
            HostHandleKind::SecretValue => "secret_value",
            HostHandleKind::BrowserSession => "browser_session",
        }
    }

    pub(crate) fn boundary_key(&self) -> String {
        match &self.payload {
            HostHandlePayload::TcpStream(stream) => {
                format!("tcp_stream:{}", stream.handle().identity_fingerprint())
            }
            HostHandlePayload::TlsStream(stream) => {
                format!("tls_stream:{}", stream.handle().identity_fingerprint())
            }
            HostHandlePayload::SecretValue(reference) => format!("secret_value:{}", reference.id()),
            HostHandlePayload::BrowserSession(id) => format!("browser_session:{id}"),
        }
    }

    pub(crate) fn byte_stream_ref(&self) -> Option<ByteStreamRef> {
        match &self.payload {
            HostHandlePayload::TcpStream(stream) => Some(stream.as_byte_stream()),
            HostHandlePayload::TlsStream(stream) => Some(stream.as_byte_stream()),
            HostHandlePayload::SecretValue(_) | HostHandlePayload::BrowserSession(_) => None,
        }
    }

    pub(crate) fn tcp_stream_ref(&self) -> Option<TcpStreamRef> {
        match &self.payload {
            HostHandlePayload::TcpStream(stream) => Some(stream.clone()),
            HostHandlePayload::TlsStream(_)
            | HostHandlePayload::SecretValue(_)
            | HostHandlePayload::BrowserSession(_) => None,
        }
    }

    pub(crate) fn secret_ref(&self) -> Option<SecretRef> {
        match &self.payload {
            HostHandlePayload::SecretValue(reference) => Some(reference.clone()),
            HostHandlePayload::TcpStream(_)
            | HostHandlePayload::TlsStream(_)
            | HostHandlePayload::BrowserSession(_) => None,
        }
    }

    pub(crate) fn browser_session_id(&self) -> Option<&str> {
        match &self.payload {
            HostHandlePayload::BrowserSession(id) => Some(id),
            HostHandlePayload::TcpStream(_)
            | HostHandlePayload::TlsStream(_)
            | HostHandlePayload::SecretValue(_) => None,
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

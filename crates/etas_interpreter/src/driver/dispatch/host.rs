use etas_host::{
    BrowserProtocolOperation, BrowserProtocolPayload, FilesystemEntry, FilesystemOperation,
    HostError, HostRequestKind, HostTraceRequest, HostValue, PolicySubject, SecretPayload,
    StreamFailure, StreamOperation, StreamPayload, StreamRead,
};

use crate::{
    control::{ControlSignal, HostBoundaryDecode, HostBoundaryRequest, PendingHostBoundary},
    eval::{EvalContext, machine::EvalMachine},
    host::HostServices,
    value::{HostHandleValue, InterpValue, ListValue, RecordValue},
};

use super::{
    error::{format_host_error, retry_or_report},
    host_dispatch::HostDispatch,
    policy::evaluate_before_boundary,
};

enum HostBoundaryFailure {
    Host(HostError),
    Stream(StreamFailure),
}

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    boundary: PendingHostBoundary,
    machine: &mut EvalMachine,
) -> Option<ControlSignal> {
    let kind = host_boundary_kind(&boundary.request);
    let key = host_boundary_key(&boundary);
    let trace_subject = host_boundary_policy_subject(&boundary.request);
    if host_boundary_is_replayable(&boundary.request)
        && let Some(value) = eval.completed_host_boundary_result(kind, &key)
    {
        return Some(eval.resume_host_signal(boundary, value));
    }
    if !evaluate_before_boundary(
        eval,
        host,
        eval.boundary_policy_ref(),
        trace_subject.clone(),
        boundary.span,
        kind,
    )
    .await
    {
        return None;
    }
    if let Err(error) = host_boundary_budget(&boundary.request).check_time() {
        return retry_or_report(
            eval,
            machine,
            boundary.continuation,
            boundary.span,
            format!("{kind} host boundary failed: {}", format_host_error(&error)),
        );
    }
    let result = match boundary.request.clone() {
        HostBoundaryRequest::Filesystem(request) => match HostDispatch::execute(
            eval,
            request.id,
            HostRequestKind::Filesystem,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            host.filesystem(request),
        )
        .await
        {
            Ok(response) => response
                .result
                .map(|entry| host_boundary_value_from_filesystem_entry(boundary.decode, entry))
                .map_err(HostBoundaryFailure::Host),
            Err(error) => Err(HostBoundaryFailure::Host(error)),
        },
        HostBoundaryRequest::Tcp(request) => match HostDispatch::execute(
            eval,
            request.id,
            HostRequestKind::Tcp,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            host.tcp(request),
        )
        .await
        {
            Ok(response) => response
                .result
                .map(|stream| {
                    let nominal_type = eval.known_std_types.tcp_stream.ok_or_else(|| {
                        "TCP response requires checked std.net.tcp.TcpStream type".to_owned()
                    })?;
                    host_boundary_value_from_tcp_stream(boundary.decode, nominal_type, stream)
                })
                .map_err(HostBoundaryFailure::Host),
            Err(error) => Err(HostBoundaryFailure::Host(error)),
        },
        HostBoundaryRequest::Stream(request) => match HostDispatch::execute(
            eval,
            request.id,
            HostRequestKind::Stream,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            host.stream(request),
        )
        .await
        {
            Ok(response) => response
                .result
                .map(|payload| host_boundary_value_from_stream_payload(boundary.decode, payload))
                .map_err(HostBoundaryFailure::Stream),
            Err(error) => Err(HostBoundaryFailure::Host(error)),
        },
        HostBoundaryRequest::Tls(request) => match HostDispatch::execute(
            eval,
            request.id,
            HostRequestKind::Tls,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            host.tls(request),
        )
        .await
        {
            Ok(response) => response
                .result
                .map(|stream| {
                    let nominal_type = eval.known_std_types.tls_stream.ok_or_else(|| {
                        "TLS response requires checked std.tls.TlsStream type".to_owned()
                    })?;
                    host_boundary_value_from_tls_stream(boundary.decode, nominal_type, stream)
                })
                .map_err(HostBoundaryFailure::Host),
            Err(error) => Err(HostBoundaryFailure::Host(error)),
        },
        HostBoundaryRequest::Secret(request) => match HostDispatch::execute(
            eval,
            request.id,
            HostRequestKind::Secret,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            host.secret(request),
        )
        .await
        {
            Ok(response) => response
                .result
                .map(|secret| {
                    host_boundary_value_from_secret(
                        boundary.decode,
                        eval.known_std_types.secret_value,
                        secret,
                    )
                })
                .map_err(HostBoundaryFailure::Host),
            Err(error) => Err(HostBoundaryFailure::Host(error)),
        },
        HostBoundaryRequest::Browser(request) => match HostDispatch::execute(
            eval,
            request.id,
            HostRequestKind::Browser,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            host.browser(request),
        )
        .await
        {
            Ok(response) => response
                .result
                .map(|payload| {
                    host_boundary_value_from_browser_payload(
                        boundary.decode,
                        eval.known_std_types.browser_session,
                        payload,
                    )
                })
                .map_err(HostBoundaryFailure::Host),
            Err(error) => Err(HostBoundaryFailure::Host(error)),
        },
    };
    match result {
        Ok(Ok(value)) => {
            if host_boundary_is_replayable(&boundary.request) {
                eval.record_completed_host_boundary(kind, key, value.clone());
            }
            Some(eval.resume_host_signal(boundary, value))
        }
        Ok(Err(message)) => retry_or_report(
            eval,
            machine,
            boundary.continuation,
            boundary.span,
            format!("{kind} host boundary failed: {message}"),
        ),
        Err(HostBoundaryFailure::Stream(failure)) => {
            Some(eval.stream_failure_signal(boundary, failure))
        }
        Err(HostBoundaryFailure::Host(error)) => {
            if let Some(signal) = eval.network_host_error_signal(boundary.clone(), error.clone()) {
                return Some(signal);
            }
            if matches!(&boundary.request, HostBoundaryRequest::Stream(_)) {
                return Some(eval.stream_failure_signal(boundary, StreamFailure::Host(error)));
            }
            retry_or_report(
                eval,
                machine,
                boundary.continuation,
                boundary.span,
                format!("{kind} host boundary failed: {}", format_host_error(&error)),
            )
        }
    }
}

fn host_boundary_budget(request: &HostBoundaryRequest) -> &etas_host::ExecutionBudget {
    match request {
        HostBoundaryRequest::Filesystem(request) => &request.budget,
        HostBoundaryRequest::Tcp(request) => &request.budget,
        HostBoundaryRequest::Stream(request) => &request.budget,
        HostBoundaryRequest::Tls(request) => &request.budget,
        HostBoundaryRequest::Secret(request) => &request.budget,
        HostBoundaryRequest::Browser(request) => &request.budget,
    }
}

pub(in crate::driver) fn host_boundary_kind(request: &HostBoundaryRequest) -> &'static str {
    match request {
        HostBoundaryRequest::Filesystem(_) => "filesystem",
        HostBoundaryRequest::Tcp(_) => "tcp",
        HostBoundaryRequest::Stream(_) => "stream",
        HostBoundaryRequest::Tls(_) => "tls",
        HostBoundaryRequest::Secret(_) => "secret",
        HostBoundaryRequest::Browser(_) => "browser",
    }
}

pub(in crate::driver) fn host_boundary_is_replayable(request: &HostBoundaryRequest) -> bool {
    !matches!(
        request,
        HostBoundaryRequest::Tcp(_) | HostBoundaryRequest::Stream(_) | HostBoundaryRequest::Tls(_)
    )
}

pub(in crate::driver) fn host_boundary_key(boundary: &PendingHostBoundary) -> String {
    match &boundary.request {
        HostBoundaryRequest::Filesystem(request) => {
            format!("filesystem:{:?}:{:?}", request.operation, boundary.decode)
        }
        HostBoundaryRequest::Tcp(request) => {
            format!("tcp:{:?}:{:?}", request.operation, boundary.decode)
        }
        HostBoundaryRequest::Stream(request) => {
            format!("stream:{:?}:{:?}", request.operation, boundary.decode)
        }
        HostBoundaryRequest::Tls(request) => {
            format!("tls:{:?}:{:?}", request.operation, boundary.decode)
        }
        HostBoundaryRequest::Secret(request) => {
            format!("secret:{:?}:{:?}", request.operation, boundary.decode)
        }
        HostBoundaryRequest::Browser(request) => {
            format!("browser:{:?}:{:?}", request.operation, boundary.decode)
        }
    }
}

pub(in crate::driver) fn host_boundary_value_from_filesystem_entry(
    decode: HostBoundaryDecode,
    entry: FilesystemEntry,
) -> Result<InterpValue, String> {
    match (decode, entry) {
        (HostBoundaryDecode::Bytes, FilesystemEntry::Bytes(bytes)) => Ok(InterpValue::Bytes(bytes)),
        (HostBoundaryDecode::PathList, FilesystemEntry::Entries(entries)) => {
            Ok(InterpValue::List(ListValue::new(
                entries
                    .into_iter()
                    .map(InterpValue::WorkspacePath)
                    .collect(),
            )))
        }
        (HostBoundaryDecode::Unit, FilesystemEntry::Unit) => Ok(InterpValue::Unit),
        (HostBoundaryDecode::FilesystemStat, FilesystemEntry::Stat(stat)) => {
            Ok(InterpValue::Record(RecordValue::new(vec![
                ("is_file".to_owned(), InterpValue::Bool(stat.is_file)),
                ("is_dir".to_owned(), InterpValue::Bool(stat.is_dir)),
                (
                    "len".to_owned(),
                    InterpValue::Number(crate::value::NumericValue::U64(stat.len)),
                ),
            ])))
        }
        (decode, entry) => Err(format!(
            "filesystem response payload {:?} does not match decode {:?}",
            entry, decode
        )),
    }
}

pub(in crate::driver) fn host_boundary_value_from_tcp_stream(
    decode: HostBoundaryDecode,
    nominal_type: etas_types::TypeId,
    stream: etas_host::TcpStreamRef,
) -> Result<InterpValue, String> {
    match decode {
        HostBoundaryDecode::TcpStream => Ok(InterpValue::HostHandle(HostHandleValue::tcp_stream(
            nominal_type,
            stream,
        ))),
        decode => Err(format!(
            "tcp stream response does not match decode {:?}",
            decode
        )),
    }
}

pub(in crate::driver) fn host_boundary_value_from_stream_payload(
    decode: HostBoundaryDecode,
    payload: StreamPayload,
) -> Result<InterpValue, String> {
    match (decode, payload) {
        (HostBoundaryDecode::StreamRead, StreamPayload::Read(StreamRead::Data(bytes))) => {
            Ok(InterpValue::Variant {
                name: "Data".to_owned(),
                fields: vec![InterpValue::Bytes(bytes)],
            })
        }
        (HostBoundaryDecode::StreamRead, StreamPayload::Read(StreamRead::Eof)) => {
            Ok(InterpValue::Variant {
                name: "Eof".to_owned(),
                fields: Vec::new(),
            })
        }
        (HostBoundaryDecode::StreamBytes, StreamPayload::Read(StreamRead::Data(bytes))) => {
            Ok(InterpValue::Bytes(bytes))
        }
        (HostBoundaryDecode::StreamBytes, StreamPayload::Read(StreamRead::Eof)) => {
            Ok(InterpValue::Bytes(Vec::new()))
        }
        (HostBoundaryDecode::Unit, StreamPayload::Unit) => Ok(InterpValue::Unit),
        (decode, payload) => Err(format!(
            "stream response payload {:?} does not match decode {:?}",
            payload, decode
        )),
    }
}

pub(in crate::driver) fn host_boundary_value_from_tls_stream(
    decode: HostBoundaryDecode,
    nominal_type: etas_types::TypeId,
    stream: etas_host::TlsStreamRef,
) -> Result<InterpValue, String> {
    match decode {
        HostBoundaryDecode::TlsStream => Ok(InterpValue::HostHandle(HostHandleValue::tls_stream(
            nominal_type,
            stream,
        ))),
        decode => Err(format!(
            "tls stream response does not match decode {:?}",
            decode
        )),
    }
}

pub(in crate::driver) fn host_boundary_value_from_secret(
    decode: HostBoundaryDecode,
    nominal_type: Option<etas_types::TypeId>,
    payload: SecretPayload,
) -> Result<InterpValue, String> {
    match (decode, payload) {
        (HostBoundaryDecode::SecretValue, SecretPayload::Value(secret)) => {
            let nominal_type = nominal_type.ok_or_else(|| {
                "secret response requires checked std.secret.SecretValue type".to_owned()
            })?;
            Ok(InterpValue::HostHandle(HostHandleValue::secret_value(
                nominal_type,
                secret,
            )))
        }
        (HostBoundaryDecode::SecretBytes, SecretPayload::Bytes(bytes)) => {
            Ok(InterpValue::Bytes(bytes))
        }
        (decode, payload) => Err(format!(
            "secret response payload {:?} does not match decode {:?}",
            payload, decode
        )),
    }
}

pub(in crate::driver) fn host_boundary_value_from_browser_payload(
    decode: HostBoundaryDecode,
    nominal_type: Option<etas_types::TypeId>,
    payload: BrowserProtocolPayload,
) -> Result<InterpValue, String> {
    match (decode, payload) {
        (HostBoundaryDecode::BrowserPayload, BrowserProtocolPayload::Session { id }) => {
            let nominal_type = nominal_type.ok_or_else(|| {
                "browser response requires checked std.browser.protocol.BrowserSession type"
                    .to_owned()
            })?;
            Ok(InterpValue::HostHandle(HostHandleValue::browser_session(
                nominal_type,
                id,
            )))
        }
        (HostBoundaryDecode::BrowserPayload, BrowserProtocolPayload::Message(bytes)) => {
            Ok(InterpValue::Record(RecordValue::new(vec![
                ("kind".to_owned(), InterpValue::String("Message".to_owned())),
                ("body".to_owned(), InterpValue::Bytes(bytes)),
            ])))
        }
        (HostBoundaryDecode::BrowserPayload, BrowserProtocolPayload::Screenshot(bytes)) => {
            Ok(InterpValue::Record(RecordValue::new(vec![
                (
                    "kind".to_owned(),
                    InterpValue::String("Screenshot".to_owned()),
                ),
                ("body".to_owned(), InterpValue::Bytes(bytes)),
            ])))
        }
        (HostBoundaryDecode::Unit, BrowserProtocolPayload::Unit) => Ok(InterpValue::Unit),
        (decode, payload) => Err(format!(
            "browser response payload {:?} does not match decode {:?}",
            payload, decode
        )),
    }
}

pub(in crate::driver) fn host_boundary_policy_subject(
    request: &HostBoundaryRequest,
) -> PolicySubject {
    match request {
        HostBoundaryRequest::Filesystem(request) => filesystem_policy_subject(request),
        HostBoundaryRequest::Tcp(request) => tcp_policy_subject(request),
        HostBoundaryRequest::Stream(request) => stream_policy_subject(request),
        HostBoundaryRequest::Tls(request) => tls_policy_subject(request),
        HostBoundaryRequest::Secret(request) => secret_policy_subject(request),
        HostBoundaryRequest::Browser(request) => browser_policy_subject(request),
    }
}

pub(in crate::driver) fn filesystem_policy_subject(
    request: &etas_host::FilesystemRequest,
) -> PolicySubject {
    let (operation, action, resource) = match &request.operation {
        FilesystemOperation::Read { path } => ("read", "Fs.read", format!("{path:?}")),
        FilesystemOperation::Write { path, .. } => ("write", "Fs.write", format!("{path:?}")),
        FilesystemOperation::Delete { path } => {
            ("delete", "HostFilesystem.delete", format!("{path:?}"))
        }
        FilesystemOperation::ReadDir { path } => ("list", "Fs.list", format!("{path:?}")),
        FilesystemOperation::Stat { path } => ("stat", "Fs.stat", format!("{path:?}")),
        FilesystemOperation::AtomicReplace { path, .. } => {
            ("atomic_replace", "Fs.atomic_replace", format!("{path:?}"))
        }
    };
    PolicySubject {
        kind: "filesystem".to_owned(),
        attributes: vec![
            (
                "action_kind".to_owned(),
                HostValue::String("filesystem".to_owned()),
            ),
            (
                "qualified_action".to_owned(),
                HostValue::String(action.to_owned()),
            ),
            (
                "operation".to_owned(),
                HostValue::String(operation.to_owned()),
            ),
            ("resource".to_owned(), HostValue::String(resource)),
        ],
    }
}

pub(in crate::driver) fn tcp_policy_subject(
    request: &etas_host::TcpConnectRequest,
) -> PolicySubject {
    let (operation, resource) = match &request.operation {
        etas_host::TcpConnectOperation::Connect { endpoint } => {
            ("connect", format!("{}:{}", endpoint.host, endpoint.port))
        }
    };
    PolicySubject {
        kind: "tcp".to_owned(),
        attributes: vec![
            (
                "action_kind".to_owned(),
                HostValue::String("network".to_owned()),
            ),
            (
                "qualified_action".to_owned(),
                HostValue::String("Net.tcp_connect".to_owned()),
            ),
            (
                "operation".to_owned(),
                HostValue::String(operation.to_owned()),
            ),
            ("resource".to_owned(), HostValue::String(resource)),
        ],
    }
}

pub(in crate::driver) fn stream_policy_subject(
    request: &etas_host::StreamRequest,
) -> PolicySubject {
    let (operation, action, resource, origin) = match &request.operation {
        StreamOperation::Read { stream, .. } => (
            "read",
            "Stream.read",
            stream.handle().identity_fingerprint(),
            stream_origin_label(stream),
        ),
        StreamOperation::ReadUntilLimit { stream, .. } => (
            "read_until_limit",
            "Stream.read",
            stream.handle().identity_fingerprint(),
            stream_origin_label(stream),
        ),
        StreamOperation::WriteAll { stream, .. } => (
            "write_all",
            "Stream.write",
            stream.handle().identity_fingerprint(),
            stream_origin_label(stream),
        ),
        StreamOperation::Flush { stream } => (
            "flush",
            "Stream.flush",
            stream.handle().identity_fingerprint(),
            stream_origin_label(stream),
        ),
        StreamOperation::Close { stream } => (
            "close",
            "Stream.close",
            stream.handle().identity_fingerprint(),
            stream_origin_label(stream),
        ),
    };
    PolicySubject {
        kind: "stream".to_owned(),
        attributes: vec![
            (
                "action_kind".to_owned(),
                HostValue::String("stream".to_owned()),
            ),
            (
                "qualified_action".to_owned(),
                HostValue::String(action.to_owned()),
            ),
            (
                "operation".to_owned(),
                HostValue::String(operation.to_owned()),
            ),
            ("resource".to_owned(), HostValue::String(resource)),
            ("origin".to_owned(), HostValue::String(origin)),
        ],
    }
}

pub(in crate::driver) fn stream_origin_label(stream: &etas_host::ByteStreamRef) -> String {
    match stream.origin() {
        etas_host::ByteStreamOrigin::Tcp { host, port } => format!("tcp:{host}:{port}"),
        etas_host::ByteStreamOrigin::Tls {
            host,
            port,
            server_name,
        } => format!(
            "tls:{host}:{port}:{}",
            server_name.as_deref().unwrap_or("<default>")
        ),
        etas_host::ByteStreamOrigin::File { path } => format!("file:{path}"),
        etas_host::ByteStreamOrigin::Browser { session } => format!("browser:{session}"),
        etas_host::ByteStreamOrigin::Opaque => "opaque".to_owned(),
    }
}

pub(in crate::driver) fn tls_policy_subject(
    request: &etas_host::TlsConnectRequest,
) -> PolicySubject {
    let (operation, resource) = match &request.operation {
        etas_host::TlsConnectOperation::Connect { server_name, .. } => {
            ("connect", server_name.clone())
        }
    };
    PolicySubject {
        kind: "tls".to_owned(),
        attributes: vec![
            (
                "action_kind".to_owned(),
                HostValue::String("tls".to_owned()),
            ),
            (
                "qualified_action".to_owned(),
                HostValue::String("Tls.handshake".to_owned()),
            ),
            (
                "operation".to_owned(),
                HostValue::String(operation.to_owned()),
            ),
            ("resource".to_owned(), HostValue::String(resource)),
        ],
    }
}

pub(in crate::driver) fn secret_policy_subject(
    request: &etas_host::SecretRequest,
) -> PolicySubject {
    let (operation, resource, qualified_action) = match &request.operation {
        etas_host::SecretOperation::Read { key } => ("read", key.clone(), "Secret.read"),
        etas_host::SecretOperation::HmacSha256 { key, .. } => {
            ("hmac_sha256", key.id().to_owned(), "Secret.use")
        }
    };
    PolicySubject {
        kind: "secret".to_owned(),
        attributes: vec![
            (
                "action_kind".to_owned(),
                HostValue::String("secret".to_owned()),
            ),
            (
                "qualified_action".to_owned(),
                HostValue::String(qualified_action.to_owned()),
            ),
            (
                "operation".to_owned(),
                HostValue::String(operation.to_owned()),
            ),
            ("resource".to_owned(), HostValue::String(resource)),
        ],
    }
}

pub(in crate::driver) fn browser_policy_subject(
    request: &etas_host::BrowserProtocolRequest,
) -> PolicySubject {
    let (operation, resource) = match &request.operation {
        BrowserProtocolOperation::Attach { profile } => ("attach", profile.clone()),
        BrowserProtocolOperation::Create { profile } => ("create", profile.clone()),
        BrowserProtocolOperation::Send { session, .. } => ("send", session.clone()),
        BrowserProtocolOperation::Recv { session, .. } => ("recv", session.clone()),
        BrowserProtocolOperation::Screenshot { session, .. } => ("screenshot", session.clone()),
        BrowserProtocolOperation::Close { session } => ("close", session.clone()),
    };
    PolicySubject {
        kind: "browser".to_owned(),
        attributes: vec![
            (
                "action_kind".to_owned(),
                HostValue::String("browser".to_owned()),
            ),
            (
                "qualified_action".to_owned(),
                HostValue::String(format!("Browser.{operation}")),
            ),
            (
                "operation".to_owned(),
                HostValue::String(operation.to_owned()),
            ),
            ("resource".to_owned(), HostValue::String(resource)),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_boundaries_create_sealed_capability_values() {
        let tcp = host_boundary_value_from_tcp_stream(
            HostBoundaryDecode::TcpStream,
            etas_types::TypeId(101),
            etas_host::TcpStreamRef::issued(
                etas_host::StreamHandleRef::issued("tcp-1", 7),
                etas_host::ByteStreamOrigin::Tcp {
                    host: "example.test".to_owned(),
                    port: 443,
                },
            ),
        )
        .expect("TCP response should decode");
        let InterpValue::HostHandle(tcp) = tcp else {
            panic!("TCP response must produce a sealed host handle");
        };
        assert_eq!(tcp.kind_name(), "tcp_stream");
        assert_eq!(tcp.nominal_type(), etas_types::TypeId(101));
        assert_eq!(
            tcp.byte_stream_ref()
                .expect("TCP is a byte stream")
                .handle()
                .generation(),
            7
        );

        let secret = host_boundary_value_from_secret(
            HostBoundaryDecode::SecretValue,
            Some(etas_types::TypeId(102)),
            etas_host::SecretPayload::Value(etas_host::SecretValue::new(
                etas_host::SecretRef::new("secret-1"),
                "<redacted>",
            )),
        )
        .expect("secret response should decode");
        let InterpValue::HostHandle(secret) = secret else {
            panic!("secret response must produce a sealed host handle");
        };
        assert_eq!(secret.kind_name(), "secret_value");
        assert_eq!(
            secret
                .secret_ref()
                .expect("secret handle should bind a ref")
                .id(),
            "secret-1"
        );

        let browser = host_boundary_value_from_browser_payload(
            HostBoundaryDecode::BrowserPayload,
            Some(etas_types::TypeId(103)),
            BrowserProtocolPayload::Session {
                id: "browser-1".to_owned(),
            },
        )
        .expect("browser response should decode");
        let InterpValue::HostHandle(browser) = browser else {
            panic!("browser response must produce a sealed host handle");
        };
        assert_eq!(browser.kind_name(), "browser_session");
        assert_eq!(browser.browser_session_id(), Some("browser-1"));
    }

    #[test]
    fn stream_policy_subject_uses_canonical_action_and_origin() {
        let read_subject = stream_policy_subject(&etas_host::StreamRequest {
            id: etas_host::HostRequestId(1),
            operation: etas_host::StreamOperation::ReadUntilLimit {
                stream: etas_host::ByteStreamRef::issued(
                    etas_host::StreamHandleRef::issued("tcp-test", 0),
                    etas_host::ByteStreamOrigin::Tcp {
                        host: "example.test".to_owned(),
                        port: 443,
                    },
                ),
                limit_bytes: 1024,
                timeout_ms: None,
            },
            authority: etas_host::AuthorityContext::deny_all(),
            trace: etas_host::TraceContext::root(etas_host::TraceId(1)),
            budget: Default::default(),
        });
        assert_eq!(attribute(&read_subject, "qualified_action"), "Stream.read");
        assert_eq!(attribute(&read_subject, "origin"), "tcp:example.test:443");

        let write_subject = stream_policy_subject(&etas_host::StreamRequest {
            id: etas_host::HostRequestId(2),
            operation: etas_host::StreamOperation::WriteAll {
                stream: etas_host::ByteStreamRef::issued(
                    etas_host::StreamHandleRef::issued("tls-test", 1),
                    etas_host::ByteStreamOrigin::Tls {
                        host: "example.test".to_owned(),
                        port: 443,
                        server_name: Some("example.test".to_owned()),
                    },
                ),
                body: b"GET / HTTP/1.1\r\n\r\n".to_vec(),
            },
            authority: etas_host::AuthorityContext::deny_all(),
            trace: etas_host::TraceContext::root(etas_host::TraceId(2)),
            budget: Default::default(),
        });
        assert_eq!(
            attribute(&write_subject, "qualified_action"),
            "Stream.write"
        );
        assert_eq!(
            attribute(&write_subject, "origin"),
            "tls:example.test:443:example.test"
        );
    }

    fn attribute(subject: &PolicySubject, name: &str) -> String {
        subject
            .attributes
            .iter()
            .find_map(|(key, value)| {
                (key == name).then(|| match value {
                    HostValue::String(value) => value.clone(),
                    other => panic!("attribute `{name}` should be a string, got {other:?}"),
                })
            })
            .unwrap_or_else(|| panic!("attribute `{name}` should exist"))
    }
}

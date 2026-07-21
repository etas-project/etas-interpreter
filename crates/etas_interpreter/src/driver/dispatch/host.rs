use etas_host::{
    BrowserProtocolOperation, BrowserProtocolPayload, FilesystemEntry, FilesystemOperation,
    HostValue, PolicySubject, SecretPayload, StreamOperation, StreamPayload, StreamRead,
};

use crate::{
    control::{ControlSignal, HostBoundaryDecode, HostBoundaryRequest, PendingHostBoundary},
    eval::{EvalContext, machine::EvalMachine},
    host::HostServices,
    value::{InterpValue, ListValue, RecordValue},
};

use super::{
    error::{format_host_error, retry_or_report},
    policy::evaluate_before_boundary,
};

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    boundary: PendingHostBoundary,
    machine: &mut EvalMachine,
) -> Option<ControlSignal> {
    let kind = host_boundary_kind(&boundary.request);
    let key = host_boundary_key(&boundary);
    if host_boundary_is_replayable(&boundary.request)
        && let Some(value) = eval.completed_host_boundary_result(kind, &key)
    {
        return Some(eval.resume_host_signal(boundary, value));
    }
    let request_id = host_boundary_request_id(&boundary.request);
    if !evaluate_before_boundary(
        eval,
        host,
        eval.boundary_policy_ref(),
        host_boundary_policy_subject(&boundary.request),
        boundary.span,
        kind,
    )
    .await
    {
        return None;
    }
    eval.record_host_request_sent(request_id);
    let result = match boundary.request.clone() {
        HostBoundaryRequest::Filesystem(request) => match host.filesystem(request).await {
            Ok(response) => {
                eval.record_host_response_received(response.id);
                response
                    .result
                    .map(|entry| host_boundary_value_from_filesystem_entry(boundary.decode, entry))
            }
            Err(error) => {
                eval.record_host_response_received(request_id);
                Err(error)
            }
        },
        HostBoundaryRequest::Tcp(request) => match host.tcp(request).await {
            Ok(response) => {
                eval.record_host_response_received(response.id);
                response
                    .result
                    .map(|stream| host_boundary_value_from_tcp_stream(boundary.decode, stream))
            }
            Err(error) => {
                eval.record_host_response_received(request_id);
                Err(error)
            }
        },
        HostBoundaryRequest::Stream(request) => match host.stream(request).await {
            Ok(response) => {
                eval.record_host_response_received(response.id);
                response.result.map(|payload| {
                    host_boundary_value_from_stream_payload(boundary.decode, payload)
                })
            }
            Err(error) => {
                eval.record_host_response_received(request_id);
                Err(error)
            }
        },
        HostBoundaryRequest::Tls(request) => match host.tls(request).await {
            Ok(response) => {
                eval.record_host_response_received(response.id);
                response
                    .result
                    .map(|stream| host_boundary_value_from_tls_stream(boundary.decode, stream))
            }
            Err(error) => {
                eval.record_host_response_received(request_id);
                Err(error)
            }
        },
        HostBoundaryRequest::Secret(request) => match host.secret(request).await {
            Ok(response) => {
                eval.record_host_response_received(response.id);
                response
                    .result
                    .map(|secret| host_boundary_value_from_secret(boundary.decode, secret))
            }
            Err(error) => {
                eval.record_host_response_received(request_id);
                Err(error)
            }
        },
        HostBoundaryRequest::Browser(request) => match host.browser(request).await {
            Ok(response) => {
                eval.record_host_response_received(response.id);
                response.result.map(|payload| {
                    host_boundary_value_from_browser_payload(boundary.decode, payload)
                })
            }
            Err(error) => {
                eval.record_host_response_received(request_id);
                Err(error)
            }
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
        Err(error) => {
            if let Some(signal) = eval.network_host_error_signal(boundary.clone(), error.clone()) {
                return Some(signal);
            }
            if let Some(signal) = eval.stream_host_error_signal(boundary.clone(), error.clone()) {
                return Some(signal);
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

pub(in crate::driver) fn host_boundary_request_id(
    request: &HostBoundaryRequest,
) -> etas_host::HostRequestId {
    match request {
        HostBoundaryRequest::Filesystem(request) => request.id,
        HostBoundaryRequest::Tcp(request) => request.id,
        HostBoundaryRequest::Stream(request) => request.id,
        HostBoundaryRequest::Tls(request) => request.id,
        HostBoundaryRequest::Secret(request) => request.id,
        HostBoundaryRequest::Browser(request) => request.id,
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
        (HostBoundaryDecode::PathList, FilesystemEntry::Entries(entries)) => Ok(InterpValue::List(
            ListValue::new(entries.into_iter().map(InterpValue::String).collect()),
        )),
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
    stream: etas_host::TcpStreamRef,
) -> Result<InterpValue, String> {
    match decode {
        HostBoundaryDecode::TcpStream => Ok(InterpValue::Record(RecordValue::new(vec![
            ("id".to_owned(), InterpValue::String(stream.id)),
            ("origin".to_owned(), stream_origin_value(stream.origin)),
        ]))),
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
    stream: etas_host::TlsStreamRef,
) -> Result<InterpValue, String> {
    match decode {
        HostBoundaryDecode::TlsStream => Ok(InterpValue::Record(RecordValue::new(vec![
            ("id".to_owned(), InterpValue::String(stream.id)),
            ("origin".to_owned(), stream_origin_value(stream.origin)),
        ]))),
        decode => Err(format!(
            "tls stream response does not match decode {:?}",
            decode
        )),
    }
}

pub(in crate::driver) fn stream_origin_value(origin: etas_host::ByteStreamOrigin) -> InterpValue {
    match origin {
        etas_host::ByteStreamOrigin::Tcp { host, port } => {
            InterpValue::Record(RecordValue::new(vec![
                ("kind".to_owned(), InterpValue::String("tcp".to_owned())),
                ("host".to_owned(), InterpValue::String(host)),
                ("port".to_owned(), InterpValue::u16(port)),
            ]))
        }
        etas_host::ByteStreamOrigin::Tls {
            host,
            port,
            server_name,
        } => {
            let mut fields = vec![
                ("kind".to_owned(), InterpValue::String("tls".to_owned())),
                ("host".to_owned(), InterpValue::String(host)),
                ("port".to_owned(), InterpValue::u16(port)),
            ];
            if let Some(server_name) = server_name {
                fields.push(("server_name".to_owned(), InterpValue::String(server_name)));
            }
            InterpValue::Record(RecordValue::new(fields))
        }
        etas_host::ByteStreamOrigin::File { path } => InterpValue::Record(RecordValue::new(vec![
            ("kind".to_owned(), InterpValue::String("file".to_owned())),
            ("path".to_owned(), InterpValue::String(path)),
        ])),
        etas_host::ByteStreamOrigin::Browser { session } => {
            InterpValue::Record(RecordValue::new(vec![
                ("kind".to_owned(), InterpValue::String("browser".to_owned())),
                ("session".to_owned(), InterpValue::String(session)),
            ]))
        }
        etas_host::ByteStreamOrigin::Opaque => InterpValue::Record(RecordValue::new(vec![(
            "kind".to_owned(),
            InterpValue::String("opaque".to_owned()),
        )])),
    }
}

pub(in crate::driver) fn host_boundary_value_from_secret(
    decode: HostBoundaryDecode,
    payload: SecretPayload,
) -> Result<InterpValue, String> {
    match (decode, payload) {
        (HostBoundaryDecode::SecretValue, SecretPayload::Value(secret)) => {
            Ok(InterpValue::Record(RecordValue::new(vec![
                (
                    "ref".to_owned(),
                    InterpValue::String(secret.reference().id().to_owned()),
                ),
                (
                    "redacted".to_owned(),
                    InterpValue::String(secret.redacted_label().to_owned()),
                ),
            ])))
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
    payload: BrowserProtocolPayload,
) -> Result<InterpValue, String> {
    match (decode, payload) {
        (HostBoundaryDecode::BrowserPayload, BrowserProtocolPayload::Session { id }) => {
            Ok(InterpValue::Record(RecordValue::new(vec![
                ("kind".to_owned(), InterpValue::String("Session".to_owned())),
                ("id".to_owned(), InterpValue::String(id)),
            ])))
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
            stream.id.clone(),
            stream_origin_label(stream),
        ),
        StreamOperation::ReadUntilLimit { stream, .. } => (
            "read_until_limit",
            "Stream.read",
            stream.id.clone(),
            stream_origin_label(stream),
        ),
        StreamOperation::WriteAll { stream, .. } => (
            "write_all",
            "Stream.write",
            stream.id.clone(),
            stream_origin_label(stream),
        ),
        StreamOperation::Flush { stream } => (
            "flush",
            "Stream.flush",
            stream.id.clone(),
            stream_origin_label(stream),
        ),
        StreamOperation::Close { stream } => (
            "close",
            "Stream.close",
            stream.id.clone(),
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
    match &stream.origin {
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
    fn stream_policy_subject_uses_canonical_action_and_origin() {
        let read_subject = stream_policy_subject(&etas_host::StreamRequest {
            id: etas_host::HostRequestId(1),
            operation: etas_host::StreamOperation::ReadUntilLimit {
                stream: etas_host::ByteStreamRef::new(
                    "tcp:example.test:443:1",
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
                stream: etas_host::ByteStreamRef::new(
                    "tls:example.test:443:example.test:2",
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

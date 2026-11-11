use std::{future::Future, pin::Pin};

use etas_host::{
    BrowserProtocolOperation, BrowserProtocolPayload, BrowserProtocolResponse, CommandProcessEvent,
    CommandProcessOperation, CommandProcessPayload, CommandProcessResponse, FilesystemEntry,
    FilesystemOperation, FilesystemResponse, HostError, HostRequestKind, HostTraceRequest,
    HostValue, PolicySubject, SecretPayload, SecretResponse, StreamFailure, StreamOperation,
    StreamPayload, StreamRead, StreamResponse, TcpConnectResponse, TlsConnectResponse,
};

use crate::{
    control::{ControlSignal, HostBoundaryDecode, HostBoundaryRequest, PendingHostBoundary},
    eval::{EvalContext, machine::EvalMachine},
    host::HostServices,
    value::{HostHandleValue, InterpValue, ListValue, RecordValue},
};

use super::{
    error::{format_host_error, retry_or_report},
    host_dispatch::{HostCompletion, HostDispatch, PendingHostOperation, TraceableHostResponse},
    policy::authorize_before_boundary,
};

enum HostBoundaryFailure {
    Host(HostError),
    Stream(StreamFailure),
    FileReadLimit {
        max_bytes: usize,
    },
    DirectoryListLimit {
        max_entries: usize,
        max_bytes: usize,
    },
}

pub(in crate::driver) enum HostBoundaryDispatchStart<'host> {
    Complete(Option<ControlSignal>),
    Pending(PendingHostBoundaryDispatch<'host>),
}

pub(in crate::driver) struct PendingHostBoundaryDispatch<'host> {
    boundary: PendingHostBoundary,
    key: Option<etas_host::replay::ReplayFingerprint>,
    operation: etas_host::execution::OperationId,
    future: Pin<Box<dyn Future<Output = HostBoundaryCompletion> + 'host>>,
}

pub(in crate::driver) struct CompletedHostBoundaryDispatch {
    boundary: PendingHostBoundary,
    key: Option<etas_host::replay::ReplayFingerprint>,
    completion: HostBoundaryCompletion,
}

enum HostBoundaryCompletion {
    Filesystem(HostCompletion<FilesystemResponse>),
    CommandProcess(HostCompletion<CommandProcessResponse>),
    Tcp(HostCompletion<TcpConnectResponse>),
    Stream(HostCompletion<StreamResponse>),
    Tls(HostCompletion<TlsConnectResponse>),
    Secret(HostCompletion<SecretResponse>),
    Browser(HostCompletion<BrowserProtocolResponse>),
}

pub(in crate::driver) async fn begin<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    mut boundary: PendingHostBoundary,
    machine: &mut EvalMachine,
) -> HostBoundaryDispatchStart<'host> {
    let kind = host_boundary_kind(&boundary.request);
    let key = match host_boundary_key(eval, &boundary) {
        Ok(key) => key,
        Err(fault) => {
            return HostBoundaryDispatchStart::Complete(Some(ControlSignal::Fault(Box::new(
                fault,
            ))));
        }
    };
    let occurrence = crate::orchestration::BoundaryOccurrenceId::HostRequest(
        host_boundary_request_id(&boundary.request),
    );
    let trace_subject = match host_boundary_policy_subject(&boundary.request) {
        Ok(subject) => subject,
        Err(error) => {
            return HostBoundaryDispatchStart::Complete(Some(ControlSignal::missing_checked_fact(
                error.message,
                boundary.span,
            )));
        }
    };
    if let Some(key) = &key {
        match eval.completed_host_boundary_result(&occurrence, kind, key, boundary.span) {
            Ok(Some(value)) => {
                return HostBoundaryDispatchStart::Complete(Some(
                    eval.resume_host_signal(boundary, value),
                ));
            }
            Ok(None) => (),
            Err(fault) => {
                return HostBoundaryDispatchStart::Complete(Some(ControlSignal::Fault(Box::new(
                    fault,
                ))));
            }
        }
    }
    if let Err(error) = authorize_before_boundary(
        eval,
        host,
        eval.boundary_policy_ref(),
        trace_subject.clone(),
    )
    .await
    {
        if let Some(signal) = eval.cancellation_signal(boundary.span) {
            return HostBoundaryDispatchStart::Complete(Some(signal));
        }
        if matches!(
            &boundary.request,
            HostBoundaryRequest::MemoryWrite(_)
                | HostBoundaryRequest::SessionHistory(_)
                | HostBoundaryRequest::SessionContext(_)
        ) {
            return HostBoundaryDispatchStart::Complete(Some(
                eval.storage_error_with_continuation(error, boundary.span, boundary.continuation),
            ));
        }
        eval.diagnostics.push(etas_core::Diagnostic::analysis(
            etas_core::AnalysisDiagnosticCode::UnhandledRuntimeError,
            boundary.span,
            super::policy::policy_failure_message(kind, &error),
        ));
        return HostBoundaryDispatchStart::Complete(None);
    }
    boundary.request.bind_authority(eval.host_authority());
    if let Err(error) = host_boundary_budget(&boundary.request).check_time() {
        if matches!(
            &boundary.request,
            HostBoundaryRequest::MemoryWrite(_)
                | HostBoundaryRequest::SessionHistory(_)
                | HostBoundaryRequest::SessionContext(_)
        ) {
            return HostBoundaryDispatchStart::Complete(Some(
                eval.storage_error_with_continuation(error, boundary.span, boundary.continuation),
            ));
        }
        return HostBoundaryDispatchStart::Complete(retry_or_report(
            eval,
            machine,
            boundary.continuation,
            boundary.span,
            format!("{kind} host boundary failed: {}", format_host_error(&error)),
        ));
    }
    match boundary.request.clone() {
        HostBoundaryRequest::SessionContext(request) => HostBoundaryDispatchStart::Complete(Some(
            super::session_context::dispatch(eval, host, boundary, request).await,
        )),
        HostBoundaryRequest::SessionHistory(request) => HostBoundaryDispatchStart::Complete(Some(
            super::session_history::dispatch(eval, host, boundary, request).await,
        )),
        HostBoundaryRequest::MemoryWrite(request) => HostBoundaryDispatchStart::Complete(Some(
            super::memory_commit::dispatch(eval, host, boundary, request).await,
        )),
        HostBoundaryRequest::Filesystem(request) => match HostDispatch::begin(
            eval,
            request.id,
            HostRequestKind::Filesystem,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            |operation| host.filesystem(operation, request),
        ) {
            Ok(pending) => {
                pending_boundary(boundary, key, pending, HostBoundaryCompletion::Filesystem)
            }
            Err(error) => HostBoundaryDispatchStart::Complete(finish_result(
                eval,
                machine,
                boundary,
                key,
                Err(HostBoundaryFailure::Host(error)),
            )),
        },
        HostBoundaryRequest::CommandProcess(request) => match HostDispatch::begin(
            eval,
            request.id,
            HostRequestKind::Command,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            |operation| host.command_process(operation, request),
        ) {
            Ok(pending) => pending_boundary(
                boundary,
                key,
                pending,
                HostBoundaryCompletion::CommandProcess,
            ),
            Err(error) => HostBoundaryDispatchStart::Complete(finish_result(
                eval,
                machine,
                boundary,
                key,
                Err(HostBoundaryFailure::Host(error)),
            )),
        },
        HostBoundaryRequest::Tcp(request) => match HostDispatch::begin(
            eval,
            request.id,
            HostRequestKind::Tcp,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            |operation| host.tcp(operation, request),
        ) {
            Ok(pending) => pending_boundary(boundary, key, pending, HostBoundaryCompletion::Tcp),
            Err(error) => HostBoundaryDispatchStart::Complete(finish_result(
                eval,
                machine,
                boundary,
                key,
                Err(HostBoundaryFailure::Host(error)),
            )),
        },
        HostBoundaryRequest::Stream(request) => match HostDispatch::begin(
            eval,
            request.id,
            HostRequestKind::Stream,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            |operation| host.stream(operation, request),
        ) {
            Ok(pending) => pending_boundary(boundary, key, pending, HostBoundaryCompletion::Stream),
            Err(error) => HostBoundaryDispatchStart::Complete(finish_result(
                eval,
                machine,
                boundary,
                key,
                Err(HostBoundaryFailure::Host(error)),
            )),
        },
        HostBoundaryRequest::Tls(request) => match HostDispatch::begin(
            eval,
            request.id,
            HostRequestKind::Tls,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            |operation| host.tls(operation, request),
        ) {
            Ok(pending) => pending_boundary(boundary, key, pending, HostBoundaryCompletion::Tls),
            Err(error) => HostBoundaryDispatchStart::Complete(finish_result(
                eval,
                machine,
                boundary,
                key,
                Err(HostBoundaryFailure::Host(error)),
            )),
        },
        HostBoundaryRequest::Secret(request) => match HostDispatch::begin(
            eval,
            request.id,
            HostRequestKind::Secret,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            |operation| host.secret(operation, request),
        ) {
            Ok(pending) => pending_boundary(boundary, key, pending, HostBoundaryCompletion::Secret),
            Err(error) => HostBoundaryDispatchStart::Complete(finish_result(
                eval,
                machine,
                boundary,
                key,
                Err(HostBoundaryFailure::Host(error)),
            )),
        },
        HostBoundaryRequest::Browser(request) => match HostDispatch::begin(
            eval,
            request.id,
            HostRequestKind::Browser,
            request.trace_payload(),
            request.authority.clone(),
            request.trace.clone(),
            |operation| host.browser(operation, request),
        ) {
            Ok(pending) => {
                pending_boundary(boundary, key, pending, HostBoundaryCompletion::Browser)
            }
            Err(error) => HostBoundaryDispatchStart::Complete(finish_result(
                eval,
                machine,
                boundary,
                key,
                Err(HostBoundaryFailure::Host(error)),
            )),
        },
    }
}

pub(in crate::driver) fn finish(
    eval: &mut EvalContext<'_>,
    machine: &mut EvalMachine,
    completed: CompletedHostBoundaryDispatch,
) -> Option<ControlSignal> {
    let CompletedHostBoundaryDispatch {
        boundary,
        key,
        completion,
    } = completed;
    let result = match completion {
        HostBoundaryCompletion::Filesystem(completion) => {
            match HostDispatch::finish(eval, completion) {
                Ok(response) => match (response.result, boundary.decode) {
                    (
                        Ok(FilesystemEntry::LimitedEntries(
                            etas_host::BoundedDirectoryList::LimitExceeded {
                                max_entries,
                                max_bytes,
                            },
                        )),
                        HostBoundaryDecode::DirectoryListLimited {
                            max_entries: expected_entries,
                            max_bytes: expected_bytes,
                        },
                    ) if max_entries == expected_entries && max_bytes == expected_bytes => {
                        Err(HostBoundaryFailure::DirectoryListLimit {
                            max_entries,
                            max_bytes,
                        })
                    }
                    (
                        Ok(FilesystemEntry::LimitedRead(
                            etas_host::BoundedFileRead::LimitExceeded { max_bytes },
                        )),
                        HostBoundaryDecode::FileReadLimited {
                            max_bytes: expected,
                        },
                    ) if max_bytes == expected => {
                        Err(HostBoundaryFailure::FileReadLimit { max_bytes })
                    }
                    (Ok(entry), decode) => {
                        Ok(host_boundary_value_from_filesystem_entry(decode, entry))
                    }
                    (Err(error), _) => Err(HostBoundaryFailure::Host(error)),
                },
                Err(error) => Err(HostBoundaryFailure::Host(error)),
            }
        }
        HostBoundaryCompletion::CommandProcess(completion) => {
            match HostDispatch::finish(eval, completion) {
                Ok(response) => response
                    .result
                    .map(|payload| {
                        host_boundary_value_from_command_process(
                            boundary.decode,
                            payload,
                            eval.checked,
                            &eval.storage_limits,
                        )
                    })
                    .map_err(HostBoundaryFailure::Host),
                Err(error) => Err(HostBoundaryFailure::Host(error)),
            }
        }
        HostBoundaryCompletion::Tcp(completion) => match HostDispatch::finish(eval, completion) {
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
        HostBoundaryCompletion::Stream(completion) => {
            match HostDispatch::finish(eval, completion) {
                Ok(response) => response
                    .result
                    .map(|payload| {
                        host_boundary_value_from_stream_payload(
                            boundary.decode,
                            payload,
                            eval.checked,
                            &eval.storage_limits,
                        )
                    })
                    .map_err(HostBoundaryFailure::Stream),
                Err(error) => Err(HostBoundaryFailure::Host(error)),
            }
        }
        HostBoundaryCompletion::Tls(completion) => match HostDispatch::finish(eval, completion) {
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
        HostBoundaryCompletion::Secret(completion) => {
            match HostDispatch::finish(eval, completion) {
                Ok(response) => response
                    .result
                    .map(|secret| {
                        host_boundary_value_from_secret(
                            boundary.decode,
                            secret,
                            &eval.checked.type_store,
                        )
                    })
                    .map_err(HostBoundaryFailure::Host),
                Err(error) => Err(HostBoundaryFailure::Host(error)),
            }
        }
        HostBoundaryCompletion::Browser(completion) => {
            match HostDispatch::finish(eval, completion) {
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
            }
        }
    };
    finish_result(eval, machine, boundary, key, result)
}

fn finish_result(
    eval: &mut EvalContext<'_>,
    machine: &mut EvalMachine,
    boundary: PendingHostBoundary,
    key: Option<etas_host::replay::ReplayFingerprint>,
    result: Result<Result<InterpValue, String>, HostBoundaryFailure>,
) -> Option<ControlSignal> {
    let kind = host_boundary_kind(&boundary.request);
    let occurrence = crate::orchestration::BoundaryOccurrenceId::HostRequest(
        host_boundary_request_id(&boundary.request),
    );
    match result {
        Ok(Ok(value)) => {
            if let HostBoundaryDecode::DirectoryListLimited {
                max_entries,
                max_bytes,
            } = boundary.decode
            {
                let valid = match &boundary.request {
                    HostBoundaryRequest::Filesystem(request) => match &request.operation {
                        etas_host::FilesystemOperation::ReadDirLimited { path, .. } => {
                            crate::eval::workspace_discovery::listing_within_bounds(
                                &value,
                                path,
                                max_entries,
                                max_bytes,
                            )
                        }
                        _ => Err("bounded listing has an inconsistent request operation".into()),
                    },
                    _ => Err("bounded listing has an inconsistent boundary kind".into()),
                };
                if valid != Ok(true) {
                    return Some(ControlSignal::missing_checked_fact(
                        format!("invalid bounded directory response: {valid:?}"),
                        boundary.span,
                    ));
                }
            }
            if let Some(key) = key {
                if let Err(fault) = eval.record_completed_host_boundary(
                    occurrence,
                    kind,
                    key,
                    value.clone(),
                    boundary.span,
                ) {
                    return Some(ControlSignal::Fault(Box::new(fault)));
                }
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
        Err(HostBoundaryFailure::FileReadLimit { max_bytes }) => {
            Some(eval.raise_file_read_limit(max_bytes, boundary.continuation, boundary.span))
        }
        Err(HostBoundaryFailure::DirectoryListLimit {
            max_entries,
            max_bytes,
        }) => Some(eval.raise_directory_list_limit(
            max_entries,
            max_bytes,
            boundary.continuation,
            boundary.span,
        )),
        Err(HostBoundaryFailure::Host(error)) => {
            if matches!(&boundary.request, HostBoundaryRequest::Filesystem(_))
                && error.code == etas_host::HostErrorCode::IoFailure
            {
                return Some(eval.filesystem_io_failure_signal(boundary, error));
            }
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

fn pending_boundary<'host, Response, Call, Wrap>(
    boundary: PendingHostBoundary,
    key: Option<etas_host::replay::ReplayFingerprint>,
    pending: PendingHostOperation<Response, Call>,
    wrap: Wrap,
) -> HostBoundaryDispatchStart<'host>
where
    Response: TraceableHostResponse + 'host,
    Call: Future<Output = Result<Response, HostError>> + 'host,
    Wrap: FnOnce(HostCompletion<Response>) -> HostBoundaryCompletion + 'host,
{
    let operation = pending.operation_id();
    HostBoundaryDispatchStart::Pending(PendingHostBoundaryDispatch {
        boundary,
        key,
        operation,
        future: Box::pin(async move { wrap(HostDispatch::poll(pending).await) }),
    })
}

impl PendingHostBoundaryDispatch<'_> {
    pub(in crate::driver) fn operation_id(&self) -> etas_host::execution::OperationId {
        self.operation
    }

    pub(in crate::driver) async fn poll(self) -> CompletedHostBoundaryDispatch {
        CompletedHostBoundaryDispatch {
            boundary: self.boundary,
            key: self.key,
            completion: self.future.await,
        }
    }
}

fn host_boundary_request_id(request: &HostBoundaryRequest) -> etas_host::HostRequestId {
    match request {
        HostBoundaryRequest::SessionContext(request) => request.id,
        HostBoundaryRequest::SessionHistory(request) => request.id,
        HostBoundaryRequest::MemoryWrite(request) => request.id,
        HostBoundaryRequest::Filesystem(request) => request.id,
        HostBoundaryRequest::CommandProcess(request) => request.id,
        HostBoundaryRequest::Tcp(request) => request.id,
        HostBoundaryRequest::Stream(request) => request.id,
        HostBoundaryRequest::Tls(request) => request.id,
        HostBoundaryRequest::Secret(request) => request.id,
        HostBoundaryRequest::Browser(request) => request.id,
    }
}

fn host_boundary_budget(request: &HostBoundaryRequest) -> &etas_host::ExecutionBudget {
    match request {
        HostBoundaryRequest::SessionContext(request) => &request.budget,
        HostBoundaryRequest::SessionHistory(request) => &request.budget,
        HostBoundaryRequest::MemoryWrite(request) => &request.budget,
        HostBoundaryRequest::Filesystem(request) => &request.budget,
        HostBoundaryRequest::CommandProcess(request) => &request.budget,
        HostBoundaryRequest::Tcp(request) => &request.budget,
        HostBoundaryRequest::Stream(request) => &request.budget,
        HostBoundaryRequest::Tls(request) => &request.budget,
        HostBoundaryRequest::Secret(request) => &request.budget,
        HostBoundaryRequest::Browser(request) => &request.budget,
    }
}

pub(in crate::driver) fn host_boundary_kind(request: &HostBoundaryRequest) -> &'static str {
    match request {
        HostBoundaryRequest::SessionContext(_) => "session",
        HostBoundaryRequest::SessionHistory(_) => "session",
        HostBoundaryRequest::MemoryWrite(_) => "memory",
        HostBoundaryRequest::Filesystem(_) => "filesystem",
        HostBoundaryRequest::CommandProcess(_) => "command process",
        HostBoundaryRequest::Tcp(_) => "tcp",
        HostBoundaryRequest::Stream(_) => "stream",
        HostBoundaryRequest::Tls(_) => "tls",
        HostBoundaryRequest::Secret(_) => "secret",
        HostBoundaryRequest::Browser(_) => "browser",
    }
}

fn host_boundary_key(
    eval: &EvalContext<'_>,
    boundary: &PendingHostBoundary,
) -> Result<Option<etas_host::replay::ReplayFingerprint>, crate::control::ExecutionFault> {
    let payload = match &boundary.request {
        HostBoundaryRequest::SessionContext(_)
        | HostBoundaryRequest::SessionHistory(_)
        | HostBoundaryRequest::MemoryWrite(_)
        | HostBoundaryRequest::CommandProcess(_)
        | HostBoundaryRequest::Tcp(_)
        | HostBoundaryRequest::Stream(_)
        | HostBoundaryRequest::Tls(_) => return Ok(None),
        HostBoundaryRequest::Filesystem(request) => request.trace_payload(),
        HostBoundaryRequest::Secret(request) => request.trace_payload(),
        HostBoundaryRequest::Browser(request) => request.trace_payload(),
    };
    eval.replay_payload_fingerprint(
        &payload.with_field(
            "replay_occurrence",
            HostValue::UInt(host_boundary_request_id(&boundary.request).0 as u128),
            etas_host::HostTraceFieldSensitivity::Public,
        ),
        boundary.span,
    )
    .map(Some)
}

pub(in crate::driver) fn host_boundary_value_from_filesystem_entry(
    decode: HostBoundaryDecode,
    entry: FilesystemEntry,
) -> Result<InterpValue, String> {
    match (decode, entry) {
        (
            HostBoundaryDecode::DirectoryListLimited {
                max_entries,
                max_bytes,
            },
            FilesystemEntry::LimitedEntries(etas_host::BoundedDirectoryList::Complete(entries)),
        ) => {
            let mut bytes = 0usize;
            for entry in &entries {
                bytes = bytes
                    .checked_add(entry.relative_name().map_err(|error| error.message)?.len())
                    .ok_or("directory response size overflow")?;
            }
            if entries.len() > max_entries || bytes > max_bytes {
                return Err("Host directory response exceeded requested limits".into());
            }
            Ok(InterpValue::List(ListValue::new(
                entries
                    .into_iter()
                    .map(InterpValue::WorkspacePath)
                    .collect(),
            )))
        }
        (
            HostBoundaryDecode::FileReadLimited { max_bytes },
            FilesystemEntry::LimitedRead(etas_host::BoundedFileRead::Complete(bytes)),
        ) if bytes.len() <= max_bytes => Ok(InterpValue::Bytes(bytes.into())),
        (HostBoundaryDecode::Bytes, FilesystemEntry::Bytes(bytes)) => {
            Ok(InterpValue::Bytes(bytes.into()))
        }
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
        (decode, _) => Err(format!(
            "filesystem response payload does not match checked decode {:?}",
            decode
        )),
    }
}

pub(in crate::driver) fn host_boundary_value_from_command_process(
    decode: HostBoundaryDecode,
    payload: CommandProcessPayload,
    checked: &etas_frontend::CheckedProject,
    limits: &etas_host::StorageLimits,
) -> Result<InterpValue, String> {
    match (decode, payload) {
        (
            HostBoundaryDecode::CommandProcessHandle { result_type },
            CommandProcessPayload::Handle { id },
        ) => {
            checked.type_store.get(result_type).ok_or_else(|| {
                "command process response requires its checked CommandHandle type".to_owned()
            })?;
            Ok(InterpValue::HostHandle(HostHandleValue::command_process(
                result_type,
                id,
            )))
        }
        (
            HostBoundaryDecode::CommandProcessEvent { result_type },
            CommandProcessPayload::Event(event),
        ) => {
            let (name, fields) = match event {
                CommandProcessEvent::Output { stdout, stderr } => (
                    "Output",
                    vec![HostValue::Bytes(stdout), HostValue::Bytes(stderr)],
                ),
                CommandProcessEvent::Exited {
                    exit_code,
                    stdout,
                    stderr,
                } => (
                    "Exited",
                    vec![
                        HostValue::Int(exit_code as i128),
                        HostValue::Bytes(stdout),
                        HostValue::Bytes(stderr),
                    ],
                ),
                CommandProcessEvent::Stopped { stdout, stderr } => (
                    "Stopped",
                    vec![HostValue::Bytes(stdout), HostValue::Bytes(stderr)],
                ),
            };
            crate::eval::host_to_checked_interp_value(
                HostValue::Variant {
                    name: name.to_owned(),
                    fields,
                },
                result_type,
                checked,
                limits,
            )
        }
        (HostBoundaryDecode::Unit, CommandProcessPayload::Unit) => Ok(InterpValue::Unit),
        (decode, payload) => Err(format!(
            "command process response payload {:?} does not match decode {:?}",
            payload, decode
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
    checked: &etas_frontend::CheckedProject,
    limits: &etas_host::StorageLimits,
) -> Result<InterpValue, String> {
    match (decode, payload) {
        (
            HostBoundaryDecode::StreamRead { result_type },
            StreamPayload::Read(StreamRead::Data(bytes)),
        ) => crate::eval::host_to_checked_interp_value(
            HostValue::Variant {
                name: "Data".into(),
                fields: vec![HostValue::Bytes(bytes)],
            },
            result_type,
            checked,
            limits,
        ),
        (HostBoundaryDecode::StreamRead { result_type }, StreamPayload::Read(StreamRead::Eof)) => {
            crate::eval::host_to_checked_interp_value(
                HostValue::Variant {
                    name: "Eof".into(),
                    fields: vec![],
                },
                result_type,
                checked,
                limits,
            )
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
    payload: SecretPayload,
    store: &etas_types::TypeStore,
) -> Result<InterpValue, String> {
    match (decode, payload) {
        (HostBoundaryDecode::SecretValue { result_type }, SecretPayload::Value(secret)) => {
            store.get(result_type).ok_or_else(|| {
                "secret response requires its checked instantiated result type".to_owned()
            })?;
            Ok(InterpValue::HostHandle(HostHandleValue::secret_value(
                result_type,
                secret,
            )))
        }
        (HostBoundaryDecode::SecretBytes { result_type }, SecretPayload::Bytes(bytes)) => {
            crate::eval::host_to_typed_interp_value(HostValue::Bytes(bytes), result_type, store)
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
                (
                    "kind".to_owned(),
                    InterpValue::String("Message".to_owned().into()),
                ),
                ("body".to_owned(), InterpValue::Bytes(bytes.into())),
            ])))
        }
        (HostBoundaryDecode::BrowserPayload, BrowserProtocolPayload::Screenshot(bytes)) => {
            Ok(InterpValue::Record(RecordValue::new(vec![
                (
                    "kind".to_owned(),
                    InterpValue::String("Screenshot".to_owned().into()),
                ),
                ("body".to_owned(), InterpValue::Bytes(bytes.into())),
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
) -> Result<PolicySubject, HostError> {
    Ok(match request {
        HostBoundaryRequest::SessionContext(request) => {
            return super::session_context::policy_subject(request);
        }
        HostBoundaryRequest::SessionHistory(request) => {
            super::session_history::policy_subject(request)
        }
        HostBoundaryRequest::MemoryWrite(request) => super::memory_commit::policy_subject(request),
        HostBoundaryRequest::Filesystem(request) => filesystem_policy_subject(request),
        HostBoundaryRequest::CommandProcess(request) => command_process_policy_subject(request),
        HostBoundaryRequest::Tcp(request) => tcp_policy_subject(request),
        HostBoundaryRequest::Stream(request) => stream_policy_subject(request),
        HostBoundaryRequest::Tls(request) => tls_policy_subject(request),
        HostBoundaryRequest::Secret(request) => secret_policy_subject(request),
        HostBoundaryRequest::Browser(request) => browser_policy_subject(request),
    })
}

pub(in crate::driver) fn filesystem_policy_subject(
    request: &etas_host::FilesystemRequest,
) -> PolicySubject {
    let (operation, action, resource) = match &request.operation {
        FilesystemOperation::Read { path } | FilesystemOperation::ReadLimited { path, .. } => {
            ("read", "Fs.read", format!("{path:?}"))
        }
        FilesystemOperation::Write { path, .. } => ("write", "Fs.write", format!("{path:?}")),
        FilesystemOperation::Delete { path } => {
            ("delete", "HostFilesystem.delete", format!("{path:?}"))
        }
        FilesystemOperation::ReadDir { path }
        | FilesystemOperation::ReadDirLimited { path, .. } => {
            ("list", "Fs.list", format!("{path:?}"))
        }
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

pub(in crate::driver) fn command_process_policy_subject(
    request: &etas_host::CommandProcessRequest,
) -> PolicySubject {
    let (operation, qualified_action, resource, program, argc) = match &request.operation {
        CommandProcessOperation::Start { argv, .. } => {
            let program = argv.first().cloned().unwrap_or_default();
            (
                "start",
                "Command.start",
                program.clone(),
                Some(program),
                Some(argv.len()),
            )
        }
        CommandProcessOperation::Reconnect { process } => (
            "reconnect",
            "Command.reconnect",
            process.clone(),
            None,
            None,
        ),
        CommandProcessOperation::Read { process, .. } => {
            ("read", "Command.read", process.clone(), None, None)
        }
        CommandProcessOperation::Write { process, .. }
        | CommandProcessOperation::CloseStdin { process } => {
            ("write", "Command.write", process.clone(), None, None)
        }
        CommandProcessOperation::Stop { process } => {
            ("stop", "Command.stop", process.clone(), None, None)
        }
    };
    let mut attributes = vec![
        (
            "action_kind".to_owned(),
            HostValue::String("command".to_owned()),
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
    ];
    if let Some(program) = program {
        attributes.push(("program".to_owned(), HostValue::String(program)));
    }
    if let Some(argc) = argc {
        attributes.push(("argc".to_owned(), HostValue::UInt(argc as u128)));
    }
    PolicySubject {
        kind: "command".to_owned(),
        attributes,
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
    fn secret_digest_responses_use_the_checked_result_type() {
        let mut types = etas_types::TypeInterner::new();
        let bytes = types.primitive(etas_types::PrimitiveType::Bytes);
        let boolean = types.primitive(etas_types::PrimitiveType::Bool);
        let digest = types.intern(etas_types::Type::Nominal(etas_types::NominalTypeRef {
            read_only_erased_view: false,
            name: "std.crypto.Digest".into(),
            representation: Some(bytes),
            params: vec![],
        }));
        let result = host_boundary_value_from_secret(
            HostBoundaryDecode::SecretBytes {
                result_type: digest,
            },
            SecretPayload::Bytes(vec![7; 32]),
            types.store(),
        )
        .unwrap();
        let InterpValue::Nominal { ty, value } = result else {
            panic!("host digest lost nominal identity");
        };
        assert_eq!(ty, digest);
        assert_eq!(*value, InterpValue::Bytes(vec![7; 32].into()));
        for result_type in [boolean, etas_types::TypeId(u32::MAX)] {
            assert!(
                host_boundary_value_from_secret(
                    HostBoundaryDecode::SecretBytes { result_type },
                    SecretPayload::Bytes(vec![7; 32]),
                    types.store(),
                )
                .is_err()
            );
        }
    }

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

        let mut types = etas_types::TypeInterner::new();
        let key = types.primitive(etas_types::PrimitiveType::String);
        let constructor = types.intern(etas_types::Type::Named(etas_types::NamedTypeRef {
            source_parameter: None,
            name: "std.secret.SecretValue".into(),
        }));
        let result_type = types.intern(etas_types::Type::Applied {
            constructor: etas_types::TypeConstructorId(constructor.0),
            args: vec![key],
        });
        let secret = host_boundary_value_from_secret(
            HostBoundaryDecode::SecretValue { result_type },
            etas_host::SecretPayload::Value(etas_host::SecretValue::new(
                etas_host::SecretRef::new("secret-1"),
                "<redacted>",
            )),
            types.store(),
        )
        .expect("secret response should decode");
        let InterpValue::HostHandle(secret) = secret else {
            panic!("secret response must produce a sealed host handle");
        };
        assert_eq!(secret.kind_name(), "secret_value");
        assert_eq!(secret.nominal_type(), result_type);
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

use std::{
    io::{Read, Write},
    sync::Mutex,
};

use etas_frontend::{
    Frontend, ModulePath, ProjectEntry, ProjectSourceLoadOptions, ProjectSourceLoadScope,
    ProjectSourceLoader, SourceInput,
};
use etas_interpreter::{
    api::{self, EntryPoint, RunOptions},
    host::{HostFuture, HostServiceAvailability, HostServices},
};

struct StreamHost {
    tcp: etas_host::LocalTcpClient,
    stream: etas_host::LocalStreamClient,
    requests: Mutex<Vec<etas_host::StreamRequest>>,
    reader_ready: tokio::sync::watch::Sender<bool>,
    reads: std::sync::atomic::AtomicUsize,
}

macro_rules! unused_host {
    ($($method:ident: $request:path => $response:path),* $(,)?) => {$(
        fn $method<'a>(&'a self, _: etas_host::execution::OperationContext, _: $request)
            -> HostFuture<'a, Result<$response, etas_host::HostError>> {
            panic!("unexpected {} host request", stringify!($method))
        }
    )*};
}

impl HostServices for StreamHost {
    fn availability(&self) -> HostServiceAvailability {
        HostServiceAvailability {
            tcp: true,
            stream: true,
            ..Default::default()
        }
    }
    fn tcp<'a>(
        &'a self,
        operation: etas_host::execution::OperationContext,
        request: etas_host::TcpConnectRequest,
    ) -> HostFuture<'a, Result<etas_host::TcpConnectResponse, etas_host::HostError>> {
        Box::pin(async move { self.tcp.execute_scoped(request, &operation).await })
    }
    fn stream<'a>(
        &'a self,
        operation: etas_host::execution::OperationContext,
        mut request: etas_host::StreamRequest,
    ) -> HostFuture<'a, Result<etas_host::StreamResponse, etas_host::HostError>> {
        Box::pin(async move {
            if matches!(
                request.operation,
                etas_host::StreamOperation::WriteAll { .. }
            ) {
                let mut ready = self.reader_ready.subscribe();
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    ready.wait_for(|ready| *ready),
                )
                .await
                .expect("join must dispatch the reader while its writer is waiting")
                .unwrap();
            }
            self.requests.lock().unwrap().push(request.clone());
            if let etas_host::StreamOperation::Read { max_bytes, .. } = &mut request.operation
                && *max_bytes > 0
            {
                let read = self.reads.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                if read == 1 {
                    use etas_host::StreamClient;
                    let mut probe = request.clone();
                    let etas_host::StreamOperation::Read { stream, .. } = &request.operation else {
                        unreachable!()
                    };
                    probe.operation = etas_host::StreamOperation::Read {
                        stream: etas_host::ByteStreamRef::issued(
                            stream.handle().clone(),
                            stream.origin().clone(),
                        ),
                        max_bytes: 1,
                        timeout_ms: Some(1000),
                    };
                    let result = self.stream.execute(probe).await.unwrap();
                    assert_eq!(
                        result.result,
                        Err(etas_host::StreamFailure::Busy),
                        "read_exact must keep admission between source chunks"
                    );
                }
                let etas_host::StreamOperation::Read { max_bytes, .. } = &mut request.operation
                else {
                    unreachable!()
                };
                *max_bytes = (*max_bytes).min(2);
            }
            if matches!(request.operation, etas_host::StreamOperation::Read { max_bytes, .. } if max_bytes > 0)
            {
                self.reader_ready.send_replace(true);
            }
            self.stream.execute_scoped(request, &operation).await
        })
    }
    unused_host! {
        console: etas_host::console::ConsoleRequest => etas_host::console::ConsoleResponse,
        compiler: etas_host::compiler::CompilerRequest => etas_host::compiler::CompilerResponse,
        model: etas_host::ModelRequest => etas_host::ModelResponse,
        tool: etas_host::ToolRequest => etas_host::ToolResponse,
        memory: etas_host::MemoryRequest => etas_host::MemoryResponse,
        memory_write: etas_host::memory::MemoryWriteRequest => etas_host::memory::MemoryWriteResponse,
        session: etas_host::SessionRequest => etas_host::SessionResponse,
        session_write: etas_host::session::SessionWriteRequest => etas_host::session::SessionWriteResponse,
        filesystem: etas_host::FilesystemRequest => etas_host::FilesystemResponse,
        command: etas_host::CommandRequest => etas_host::CommandResponse,
        command_process: etas_host::CommandProcessRequest => etas_host::CommandProcessResponse,
        tls: etas_host::TlsConnectRequest => etas_host::TlsConnectResponse,
        secret: etas_host::SecretRequest => etas_host::SecretResponse,
        browser: etas_host::BrowserProtocolRequest => etas_host::BrowserProtocolResponse,
        approval: etas_host::ApprovalRequest => etas_host::ApprovalResponse,
        policy: etas_host::PolicyEvaluationRequest => etas_host::PolicyResponse,
    }
}

#[test]
fn source_stream_algorithms_use_duration_timeouts_and_actual_tcp_io() {
    let root = etas_std::library::source_checkout_manifest()
        .parent()
        .unwrap()
        .to_path_buf();
    let mut input = ProjectSourceLoader::default()
        .load(ProjectSourceLoadOptions {
            project_root: root.clone(),
            roots: vec![root.join("library")],
            entry: ProjectEntry {
                module: Some(ModulePath {
                    segments: vec!["app".into(), "stream".into()],
                }),
                flow: "verify".into(),
            },
            scope: ProjectSourceLoadScope::FullSourceTree,
            source_kind: None,
            import_search_roots: vec![],
            external_module_roots: vec![],
        })
        .unwrap()
        .input;
    let mut source = SourceInput::anonymous(
        r#"
module app.stream;
flow verify(port: i32) -> bytes {
    let host = std.net.tcp.Host { host= "127.0.0.1" };
    let stream = std.net.tcp.connect(host, std.net.tcp.Port { port= port }, std.net.tcp.TcpOptions {});
    let timeout: std.stream.Timeout = std.runtime.time.Duration.from_millis(5000);
    std.core.assert(std.stream.read_exact(stream, 0).length == 0);
    let (head, sent) = std.task.join((
        () => std.stream.read_exact(stream, 5, Some(timeout)),
        () => {
            std.stream.write_all(stream, "ping".utf8, Some(timeout));
            std.stream.flush(stream, Some(timeout));
            std.stream.flush(stream);
        },
    ));
    std.core.assert(head == "hello".utf8);
    let tail = std.stream.read_until_limit(stream, std.stream.ByteLimit { bytes= 5 }, Some(timeout));
    std.core.assert(tail == "world".utf8);
    match std.stream.read_exact(stream, 1)? {
        Err(std.stream.StreamError.UnexpectedEof) => (),
        _ => std.core.abort("read_exact lost its EOF error"),
    };
    std.stream.close(stream);
    std.stream.close(stream);
    return head + tail;
}
"#,
    );
    source.path = Some(root.join("source-stream-verification.es"));
    source.id.0 = input
        .sources
        .iter()
        .map(|source| source.id.0)
        .max()
        .unwrap()
        + 1;
    input.sources.push(source);
    input.environment.local_source_package =
        Some(etas_code::PackageKey::from_name_and_version("std", "0.5.0").unwrap());
    let output = Frontend.check_standard_library_source(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let entry = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| {
            (checked.hir.declaration_path(item.symbol()?)? == ["app", "stream", "verify"])
                .then_some(id)
        })
        .unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut connection, _) = listener.accept().unwrap();
        connection
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut request = [0; 4];
        connection.read_exact(&mut request).unwrap();
        assert_eq!(&request, b"ping");
        connection.write_all(b"he").unwrap();
        connection.write_all(b"lloworld").unwrap();
        connection.shutdown(std::net::Shutdown::Write).unwrap();
    });
    let streams = etas_host::ByteStreamStore::new();
    let host = StreamHost {
        tcp: etas_host::LocalTcpClient::new(streams.clone()),
        stream: etas_host::LocalStreamClient::new(streams),
        requests: Default::default(),
        reader_ready: tokio::sync::watch::channel(false).0,
        reads: Default::default(),
    };
    let mut options = RunOptions::default();
    options
        .host_context
        .authority
        .grants
        .push(etas_host::HostActionGrant::allow("Net", "tcp_connect"));
    for action in ["read", "write", "flush", "close"] {
        options
            .host_context
            .authority
            .grants
            .push(etas_host::HostActionGrant::allow("Stream", action));
    }
    options.host_context.authority.sandbox = etas_host::SandboxPolicy::allow_listed(
        etas_host::FilesystemPolicy::deny_all(),
        etas_host::NetworkPolicy::allow_endpoints(vec![etas_host::NetworkEndpoint::new(
            "tcp",
            "127.0.0.1",
            port,
        )]),
        etas_host::CommandPolicy::deny_all(),
        etas_host::DestructiveOpPolicy::deny_all(),
    );
    let result = api::run_checked_blocking(
        &checked,
        EntryPoint { item: entry },
        vec![api::InterpValue::i32(port as i32)],
        &host,
        options,
    )
    .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(
        matches!(result.value(), Some(api::InterpValue::Bytes(bytes)) if bytes.as_ref() == b"helloworld"),
        "{:?}",
        result.value()
    );
    server.join().unwrap();
    let requests = host.requests.lock().unwrap();
    let read = requests
        .iter()
        .position(|request| {
            matches!(request.operation,
        etas_host::StreamOperation::Read { max_bytes, .. } if max_bytes > 0)
        })
        .unwrap();
    let write = requests
        .iter()
        .position(|request| {
            matches!(
                request.operation,
                etas_host::StreamOperation::WriteAll { .. }
            )
        })
        .unwrap();
    assert!(
        read < write,
        "join must dispatch the waiting reader before its writer"
    );
    assert!(requests.iter().any(|request| matches!(
        &request.operation,
        etas_host::StreamOperation::WriteAll {
            timeout_ms: Some(5000),
            ..
        }
    )));
    assert!(requests.iter().any(|request| matches!(
        &request.operation,
        etas_host::StreamOperation::Flush {
            timeout_ms: Some(5000),
            ..
        }
    )));
    assert!(requests.iter().any(|request| matches!(
        &request.operation,
        etas_host::StreamOperation::Flush {
            timeout_ms: None,
            ..
        }
    )));
}

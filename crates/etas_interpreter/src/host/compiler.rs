use etas_host::{
    HostError,
    compiler::{CompilerClient, CompilerRequest, CompilerResponse},
    execution::OperationContext,
};

use super::{HostFuture, HostServiceAvailability, HostServices};

/// Embedding composition only: compilation remains in the supplied managed
/// service, and publishing/executing a returned artifact remains engine-owned.
pub struct CompilerHost<H, C> {
    inner: H,
    compiler: C,
    resolver: Option<std::sync::Arc<dyn etas_frontend::code::service::CheckedCandidateResolver>>,
}

impl<H, C> CompilerHost<H, C> {
    pub fn new(inner: H, compiler: C) -> Self {
        Self {
            inner,
            compiler,
            resolver: None,
        }
    }
    pub fn with_candidate_resolver(
        mut self,
        resolver: std::sync::Arc<dyn etas_frontend::code::service::CheckedCandidateResolver>,
    ) -> Self {
        self.resolver = Some(resolver);
        self
    }
    pub fn inner(&self) -> &H {
        &self.inner
    }
    pub fn compiler_client(&self) -> &C {
        &self.compiler
    }
}

macro_rules! delegate {
    ($($method:ident: $request:path => $response:path),* $(,)?) => { $(
        fn $method<'a>(&'a self, operation: OperationContext, request: $request) -> HostFuture<'a, Result<$response, HostError>> {
            self.inner.$method(operation, request)
        }
    )* };
}

impl<H: HostServices, C: CompilerClient + Sync> HostServices for CompilerHost<H, C> {
    fn checked_candidate_resolver(
        &self,
    ) -> Option<&dyn etas_frontend::code::service::CheckedCandidateResolver> {
        self.resolver.as_deref()
    }
    fn availability(&self) -> HostServiceAvailability {
        HostServiceAvailability {
            compiler: true,
            ..self.inner.availability()
        }
    }

    fn compiler<'a>(
        &'a self,
        operation: OperationContext,
        request: CompilerRequest,
    ) -> HostFuture<'a, Result<CompilerResponse, HostError>> {
        let compiler = &self.compiler;
        Box::pin(async move { compiler.execute(request, &operation).await })
    }

    delegate! {
        model: etas_host::ModelRequest => etas_host::ModelResponse,
        tool: etas_host::ToolRequest => etas_host::ToolResponse,
        memory: etas_host::MemoryRequest => etas_host::MemoryResponse,
        memory_write: etas_host::memory::MemoryWriteRequest => etas_host::memory::MemoryWriteResponse,
        session: etas_host::SessionRequest => etas_host::SessionResponse,
        session_write: etas_host::session::SessionWriteRequest => etas_host::session::SessionWriteResponse,
        filesystem: etas_host::FilesystemRequest => etas_host::FilesystemResponse,
        command: etas_host::CommandRequest => etas_host::CommandResponse,
        tcp: etas_host::TcpConnectRequest => etas_host::TcpConnectResponse,
        stream: etas_host::StreamRequest => etas_host::StreamResponse,
        tls: etas_host::TlsConnectRequest => etas_host::TlsConnectResponse,
        secret: etas_host::SecretRequest => etas_host::SecretResponse,
        browser: etas_host::BrowserProtocolRequest => etas_host::BrowserProtocolResponse,
        console: etas_host::console::ConsoleRequest => etas_host::console::ConsoleResponse,
        approval: etas_host::ApprovalRequest => etas_host::ApprovalResponse,
        policy: etas_host::PolicyEvaluationRequest => etas_host::PolicyResponse,
    }
}

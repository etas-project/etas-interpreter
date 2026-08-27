use std::{future::Future, pin::Pin};

use etas_host::console::{ConsoleRequest, ConsoleResponse};
use etas_host::{
    ApprovalRequest, ApprovalResponse, BrowserProtocolRequest, BrowserProtocolResponse,
    CommandRequest, CommandResponse, FilesystemRequest, FilesystemResponse, MemoryRequest,
    MemoryResponse, ModelRequest, ModelResponse, PolicyEvaluationRequest, PolicyResponse,
    SecretRequest, SecretResponse, SessionRequest, SessionResponse, StreamRequest, StreamResponse,
    TcpConnectRequest, TcpConnectResponse, TlsConnectRequest, TlsConnectResponse, ToolRequest,
    ToolResponse,
};

use super::HostServiceAvailability;

pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait HostServices {
    fn availability(&self) -> HostServiceAvailability;

    fn model<'a>(
        &'a self,
        request: ModelRequest,
    ) -> HostFuture<'a, Result<ModelResponse, etas_host::HostError>>;

    fn tool<'a>(
        &'a self,
        request: ToolRequest,
    ) -> HostFuture<'a, Result<ToolResponse, etas_host::HostError>>;

    fn memory<'a>(
        &'a self,
        request: MemoryRequest,
    ) -> HostFuture<'a, Result<MemoryResponse, etas_host::HostError>>;

    fn session<'a>(
        &'a self,
        request: SessionRequest,
    ) -> HostFuture<'a, Result<SessionResponse, etas_host::HostError>>;

    fn filesystem<'a>(
        &'a self,
        request: FilesystemRequest,
    ) -> HostFuture<'a, Result<FilesystemResponse, etas_host::HostError>>;

    fn command<'a>(
        &'a self,
        request: CommandRequest,
    ) -> HostFuture<'a, Result<CommandResponse, etas_host::HostError>>;

    fn tcp<'a>(
        &'a self,
        request: TcpConnectRequest,
    ) -> HostFuture<'a, Result<TcpConnectResponse, etas_host::HostError>>;

    fn stream<'a>(
        &'a self,
        request: StreamRequest,
    ) -> HostFuture<'a, Result<StreamResponse, etas_host::HostError>>;

    fn tls<'a>(
        &'a self,
        request: TlsConnectRequest,
    ) -> HostFuture<'a, Result<TlsConnectResponse, etas_host::HostError>>;

    fn secret<'a>(
        &'a self,
        request: SecretRequest,
    ) -> HostFuture<'a, Result<SecretResponse, etas_host::HostError>>;

    fn browser<'a>(
        &'a self,
        request: BrowserProtocolRequest,
    ) -> HostFuture<'a, Result<BrowserProtocolResponse, etas_host::HostError>>;

    fn console<'a>(
        &'a self,
        request: ConsoleRequest,
    ) -> HostFuture<'a, Result<ConsoleResponse, etas_host::HostError>>;

    fn approval<'a>(
        &'a self,
        request: ApprovalRequest,
    ) -> HostFuture<'a, Result<ApprovalResponse, etas_host::HostError>>;

    fn policy<'a>(
        &'a self,
        request: PolicyEvaluationRequest,
    ) -> HostFuture<'a, Result<PolicyResponse, etas_host::HostError>>;
}

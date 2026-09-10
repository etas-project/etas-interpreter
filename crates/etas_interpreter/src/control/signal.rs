use etas_core::Span;
use etas_hir::{HirBlockId, HirExprId, HirItemId, ResolvedActionRef};
use etas_host::console::ConsoleRequest;
use etas_host::{
    BrowserProtocolRequest, CommandRequest, FilesystemRequest, MemoryRequest, ModelRequest,
    SecretRequest, SessionRequest, StreamRequest, TcpConnectRequest, TlsConnectRequest,
};

use crate::{
    control::{CallTarget, Continuation},
    value::InterpValue,
};

#[derive(Clone, Debug)]
pub enum ControlSignal {
    Value(InterpValue),
    Return(InterpValue),
    Break,
    Continue,
    Cancelled(etas_host::execution::CancellationCause),
    Apply(Box<PendingContinuation>),
    Checkpoint(Box<PendingCheckpoint>),
    Block(Box<PendingBlock>),
    Expr(Box<PendingExpr>),
    Call(Box<PendingCall>),
    Perform(Box<PendingPerform>),
    Memory(Box<PendingMemory>),
    Session(Box<PendingSession>),
    Console(Box<PendingConsole>),
    Command(Box<PendingCommand>),
    Model(Box<PendingModel>),
    Host(Box<PendingHostBoundary>),
    Resume(InterpValue),
    Finish(InterpValue),
    Fault(Box<crate::control::ExecutionFault>),
}

impl ControlSignal {
    pub fn fault(
        code: etas_core::AnalysisDiagnosticCode,
        span: Span,
        message: impl Into<String>,
    ) -> Self {
        Self::Fault(Box::new(crate::control::ExecutionFault::new(
            code, span, message,
        )))
    }

    pub fn execution_aborted(message: impl Into<String>, span: Span) -> Self {
        Self::fault(
            etas_core::AnalysisDiagnosticCode::ExecutionAborted,
            span,
            message,
        )
    }

    pub fn invalid_arguments(message: impl Into<String>, span: Span) -> Self {
        Self::fault(
            etas_core::AnalysisDiagnosticCode::InvalidArguments,
            span,
            message,
        )
    }

    pub fn missing_checked_fact(message: impl Into<String>, span: Span) -> Self {
        Self::fault(
            etas_core::AnalysisDiagnosticCode::MissingCheckedFact,
            span,
            message,
        )
    }

    pub fn runtime_fault(message: impl Into<String>, span: Span) -> Self {
        Self::fault(
            etas_core::AnalysisDiagnosticCode::UnhandledRuntimeError,
            span,
            message,
        )
    }

    pub fn pending_continuation(pending: PendingContinuation) -> Self {
        Self::Apply(Box::new(pending))
    }

    pub fn pending_checkpoint(pending: PendingCheckpoint) -> Self {
        Self::Checkpoint(Box::new(pending))
    }

    pub fn pending_block(pending: PendingBlock) -> Self {
        Self::Block(Box::new(pending))
    }

    pub fn pending_expr(pending: PendingExpr) -> Self {
        Self::Expr(Box::new(pending))
    }

    pub fn pending_call(pending: PendingCall) -> Self {
        Self::Call(Box::new(pending))
    }

    pub fn pending_perform(pending: PendingPerform) -> Self {
        Self::Perform(Box::new(pending))
    }

    pub fn pending_memory(pending: PendingMemory) -> Self {
        Self::Memory(Box::new(pending))
    }

    pub fn pending_session(pending: PendingSession) -> Self {
        Self::Session(Box::new(pending))
    }

    pub fn pending_console(pending: PendingConsole) -> Self {
        Self::Console(Box::new(pending))
    }

    pub fn pending_command(pending: PendingCommand) -> Self {
        Self::Command(Box::new(pending))
    }

    pub fn pending_model(pending: PendingModel) -> Self {
        Self::Model(Box::new(pending))
    }

    pub fn pending_host(pending: PendingHostBoundary) -> Self {
        Self::Host(Box::new(pending))
    }
}

#[derive(Clone, Debug)]
pub struct PendingContinuation {
    pub continuation: Continuation,
    pub input: ContinuationInput,
}

#[derive(Clone, Debug)]
pub enum ContinuationInput {
    Value(InterpValue),
    Return(InterpValue),
    Resume(InterpValue),
    Finish(InterpValue),
    Break,
    Continue,
}

#[derive(Clone, Debug)]
pub struct PendingCheckpoint {
    pub label: Option<String>,
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub struct PendingBlock {
    pub block: HirBlockId,
    pub next_stmt_index: usize,
    pub frame: crate::control::Frame,
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub struct PendingExpr {
    pub expr: HirExprId,
    pub frame: crate::control::Frame,
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub struct PendingCall {
    pub target: CallTarget,
    pub args: Vec<InterpValue>,
    pub span: Span,
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub struct PendingPerform {
    pub expr: Option<HirExprId>,
    pub action: ResolvedActionRef,
    pub error_type: Option<etas_types::TypeId>,
    pub args: Vec<InterpValue>,
    pub span: Span,
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub struct PendingMemory {
    pub request: MemoryRequest,
    pub decode: MemoryDecode,
    pub span: Span,
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub struct PendingSession {
    pub request: SessionRequest,
    pub decode: SessionDecode,
    pub span: Span,
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub struct PendingConsole {
    pub request: ConsoleRequest,
    pub decode: ConsoleDecode,
    pub span: Span,
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub struct PendingCommand {
    pub request: CommandRequest,
    pub decode: CommandDecode,
    pub span: Span,
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub struct PendingModel {
    pub request: ModelRequest,
    pub decode: ModelDecode,
    pub max_tool_rounds: usize,
    pub source_tools: Vec<SourceToolBinding>,
    pub span: Span,
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub struct PendingHostBoundary {
    pub request: HostBoundaryRequest,
    pub decode: HostBoundaryDecode,
    pub span: Span,
    pub continuation: Continuation,
}

#[derive(Clone, Debug)]
pub enum HostBoundaryRequest {
    SessionHistory(SessionRequest),
    SessionContext(etas_host::session::SessionWriteRequest),
    MemoryWrite(etas_host::memory::MemoryWriteRequest),
    Filesystem(FilesystemRequest),
    Tcp(TcpConnectRequest),
    Stream(StreamRequest),
    Tls(TlsConnectRequest),
    Secret(SecretRequest),
    Browser(BrowserProtocolRequest),
}

#[derive(Clone, Debug)]
pub struct SourceToolBinding {
    pub name: String,
    pub qualified_name: Option<String>,
    pub item: HirItemId,
}

#[derive(Clone, Copy, Debug)]
pub enum MemoryDecode {
    Page { result_type: etas_types::TypeId },
    Entry { result_type: etas_types::TypeId },
    OptionValue { value_type: etas_types::TypeId },
    BoolContains,
    KeyList { key_type: etas_types::TypeId },
    JsonEntries,
    Unit,
}

#[derive(Clone, Debug)]
pub enum SessionDecode {
    ResolveThenAppendMessage {
        message: crate::value::MessageValue,
    },
    ResolveThenLoadConversation {
        config: crate::value::SessionConfigValue,
        payload_type: etas_types::TypeId,
    },
    ReturnMessage {
        message: crate::value::MessageValue,
    },
    ReturnConversation {
        payload_type: etas_types::TypeId,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum ConsoleDecode {
    String,
    Unit,
}

#[derive(Clone, Copy, Debug)]
pub enum CommandDecode {
    CommandResult,
}

#[derive(Clone, Copy, Debug)]
pub enum HostBoundaryDecode {
    SessionHistory { result_type: etas_types::TypeId },
    SessionContext { result_type: etas_types::TypeId },
    Storage { result_type: etas_types::TypeId },
    Bytes,
    Unit,
    PathList,
    FilesystemStat,
    TcpStream,
    StreamRead,
    StreamBytes,
    TlsStream,
    SecretValue,
    SecretBytes,
    BrowserPayload,
}

#[derive(Clone, Copy, Debug)]
pub enum ModelDecode {
    String,
    ModelResponse,
    Typed(etas_types::TypeId),
}

#[cfg(test)]
mod size_tests {
    use super::*;

    #[test]
    fn pending_boundaries_do_not_inflate_control_signal_stack_slots() {
        assert!(
            std::mem::size_of::<ControlSignal>() <= 256,
            "ControlSignal must keep large pending boundary payloads off the Rust stack"
        );
    }
}

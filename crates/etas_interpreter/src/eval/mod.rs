mod agent;
mod aggregate_expr;
mod assign;
mod assign_nested;
mod assign_place;
mod block;
mod boundary;
mod boundary_approval;
mod boundary_command;
mod boundary_console;
mod boundary_ledger;
mod boundary_memory;
mod boundary_model;
mod boundary_network;
mod boundary_session;
mod boundary_stream;
mod branch;
mod call;
mod callee;
mod checkpoint;
mod const_expr;
mod continuation;
mod expr;
mod field_access;
mod flow;
mod handler;
mod host_value;
mod index_access;
pub(crate) mod limit;
mod loop_control;
pub(crate) mod machine;
mod memory_args;
mod memory_selection;
mod memory_store;
mod method;
mod operator_expr;
mod pattern;
mod perform;
mod pipeline;
mod slice_access;
mod spec_method;
mod std_call;
mod stmt;
mod stmt_assign;
mod stmt_bind;
mod stmt_branch;
mod stmt_retry;
mod stmt_value;
mod try_expr;
mod variant;
pub(crate) use crate::control::{
    AggregateKind, CallTarget, CommandDecode, ConsoleDecode, Continuation, ContinuationInput,
    ControlSignal, Frame, HostBoundaryDecode, HostBoundaryRequest, MemoryDecode, ModelDecode,
    PendingBlock, PendingCall, PendingCheckpoint, PendingCommand, PendingConsole,
    PendingContinuation, PendingExpr, PendingHostBoundary, PendingMemory, PendingModel,
    PendingPerform, PendingSession, SessionDecode, SourceToolBinding, StaticMethodKind,
};
pub(crate) use assign_place::{LocalPlaceComponent, LocalPlaceSegment};
use branch::IfStmtResume;
pub(crate) use host_value::interp_to_host_value;
use memory_args::MemoryArgsResume;
use memory_selection::{MemorySelectionLimitResume, MemorySelectionMethodEval};
use memory_store::{MemoryStoreArgs, MemoryStoreMethodEval};
use perform::PerformArgsResume;
pub(crate) use slice_access::SliceExprEval;

use crate::{
    CheckedProject,
    api::{ExecutionLimits, HostExecutionContext, ModelExecutionPolicy},
    diagnostics::item_span,
    intrinsic::dispatch::{
        BrowserCallable, CommandCallable, ConsoleCallable, FilesystemCallable, JsonCallable,
        SecretCallable, StdCallable, StreamCallable, TcpCallable, TlsCallable,
    },
    orchestration::{
        ActiveHandlerArmRecord, ActiveHandlerRecord, CheckpointId, HandlerScopeId, HandlerSnapshot,
        HostBoundaryLedger, InterpreterCheckpoint, ResourceVersionRecord, ResourceVersionSnapshot,
        RetryAttemptId, RetryAttemptRecord, RetrySnapshot, TraceSnapshot, WorkflowEvent,
        WorkflowStepId,
    },
    plan::{BraceLiteralShape, InterpreterPlan},
    value::{ArrayValue, InterpValue, ListValue, MapValue, RecordValue, SliceValue},
};
use etas_core::{AnalysisDiagnosticCode, Diagnostic, Span};
use etas_hir::{
    HirArg, HirBinaryOp, HirBlockId, HirEffectRef, HirElseBranch, HirExpr, HirExprId, HirFlowDecl,
    HirGenericArg, HirHandlerArm, HirItem, HirItemId, HirLiteral, HirPat, HirStmt, HirToolBody,
    HirTreeView, HirTypeId, HirUnaryOp, PartialResolutionReason, ResolveResult, ResolvedActionRef,
    SymbolDef, SymbolId, SymbolKind, TopLevelLetClassification,
};
use etas_host::console::{ConsoleOperation, ConsoleRequest};
use etas_host::{
    ApprovalGrant, ApprovalRequest, AuthorityContext, BrowserProtocolOperation,
    BrowserProtocolRequest, ByteStreamRef, CommandRequest, FilesystemOperation, FilesystemRequest,
    HostRequestId, HostValue, MemoryOperation, MemoryRegionRef, MemoryRequest, MemoryResult,
    MemoryWriteMode, ModelContent, ModelMessage, ModelRequest, ModelResponse, ModelRole,
    SecretOperation, SecretRequest, StoreRef, StreamOperation, StreamRequest, TcpConnectOperation,
    TcpConnectRequest, TcpEndpoint, TcpStreamRef, TlsConnectOperation, TlsConnectRequest,
    TraceContext, WorkspacePath,
};

pub struct EvalContext<'a> {
    pub checked: &'a CheckedProject,
    pub view: HirTreeView<'a>,
    pub plan: &'a InterpreterPlan,
    pub known_std_types: KnownStdTypes,
    pub host_context: HostExecutionContext,
    pub model_policy: ModelExecutionPolicy,
    pub execution_limits: ExecutionLimits,
    pub current_session: Option<String>,
    pub entry_item: HirItemId,
    pub entry_args: &'a [InterpValue],
    pub diagnostics: Vec<Diagnostic>,
    pub events: Vec<WorkflowEvent>,
    pub checkpoints: Vec<InterpreterCheckpoint>,
    next_step: u32,
    next_checkpoint: u32,
    next_retry: u32,
    next_handler_scope: u32,
    next_host_request: u32,
    next_message: u32,
    execution_steps: u64,
    retry_stack: Vec<RetryAttemptRecord>,
    handler_stack: Vec<ActiveHandlerRecord>,
    completed_host_boundaries: Vec<crate::orchestration::CompletedHostBoundary>,
    resource_versions: Vec<ResourceVersionRecord>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KnownStdTypes {
    pub browser_session: Option<etas_types::TypeId>,
    pub io_error: Option<etas_types::TypeId>,
    pub memory_conflict: Option<etas_types::TypeId>,
    pub memory_version: Option<etas_types::TypeId>,
    pub network_error: Option<etas_types::TypeId>,
    pub secret_value: Option<etas_types::TypeId>,
    pub stream_error: Option<etas_types::TypeId>,
    pub tcp_stream: Option<etas_types::TypeId>,
    pub tls_error: Option<etas_types::TypeId>,
    pub tls_stream: Option<etas_types::TypeId>,
}

pub struct EvalContextInput<'a> {
    pub checked: &'a CheckedProject,
    pub plan: &'a InterpreterPlan,
    pub host_context: HostExecutionContext,
    pub model_policy: ModelExecutionPolicy,
    pub execution_limits: ExecutionLimits,
    pub current_session: Option<String>,
    pub entry_item: HirItemId,
    pub entry_args: &'a [InterpValue],
}

impl<'a> EvalContext<'a> {
    pub fn new(input: EvalContextInput<'a>) -> Self {
        let EvalContextInput {
            checked,
            plan,
            host_context,
            model_policy,
            execution_limits,
            current_session,
            entry_item,
            entry_args,
        } = input;
        Self {
            checked,
            view: HirTreeView::new(&checked.hir),
            plan,
            known_std_types: KnownStdTypes::from_checked(checked),
            host_context,
            model_policy,
            execution_limits,
            current_session,
            entry_item,
            entry_args,
            diagnostics: Vec::new(),
            events: Vec::new(),
            checkpoints: Vec::new(),
            next_step: 0,
            next_checkpoint: 0,
            next_retry: 0,
            next_handler_scope: 0,
            next_host_request: 0,
            next_message: 0,
            execution_steps: 0,
            retry_stack: Vec::new(),
            handler_stack: Vec::new(),
            completed_host_boundaries: Vec::new(),
            resource_versions: Vec::new(),
        }
    }

    pub(crate) fn consume_execution_step(
        &mut self,
        span: Span,
    ) -> Result<(), crate::control::ExecutionFault> {
        if let Some(max_steps) = self.execution_limits.max_steps
            && self.execution_steps >= max_steps.get()
        {
            let message = format!(
                "maximum interpreter execution steps ({max_steps}) exceeded; this usually indicates unbounded computation"
            );
            return Err(crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                span,
                message,
            ));
        }
        self.execution_steps = self.execution_steps.saturating_add(1);
        Ok(())
    }

    pub(crate) fn host_authority(&self) -> AuthorityContext {
        self.host_context.authority.clone()
    }

    pub(crate) fn record_approval_grant(&mut self, grant: ApprovalGrant) {
        self.host_context
            .authority
            .grants
            .extend(grant.grants.iter().cloned());
        self.host_context.authority.approvals.push(grant);
    }

    pub(crate) fn host_trace(&self) -> TraceContext {
        self.host_context.trace.clone()
    }

    pub(crate) fn host_budget(&self) -> etas_host::ExecutionBudget {
        self.host_context.budget.clone()
    }

    pub(crate) fn item_primary_block(&self, item: HirItemId) -> Option<HirBlockId> {
        self.view
            .item(item)
            .and_then(|item| item.body())
            .and_then(|body| body.blocks().next())
            .map(|block| block.id())
    }

    pub(crate) fn handler_arm_body(&self, arm: etas_hir::HirHandlerArmId) -> Option<HirBlockId> {
        self.view.handler_arm(arm).map(|arm| arm.body().id())
    }

    pub(crate) fn boundary_policy_ref(&self) -> Option<HostValue> {
        self.host_context
            .authority
            .policy
            .boundary_policy_ref
            .clone()
    }

    pub(crate) fn next_host_request_id(&mut self) -> HostRequestId {
        let id = HostRequestId(self.next_host_request);
        self.next_host_request += 1;
        id
    }
}

impl KnownStdTypes {
    fn from_checked(checked: &CheckedProject) -> Self {
        Self {
            browser_session: resolve_std_type(
                checked,
                &["std", "browser", "protocol", "BrowserSession"],
            ),
            io_error: resolve_std_type(checked, &["std", "io", "IOError"]),
            memory_conflict: resolve_std_type(checked, &["std", "memory", "MemoryConflict"]),
            memory_version: resolve_std_type(checked, &["std", "memory", "MemoryVersion"]),
            network_error: resolve_std_type(checked, &["std", "net", "tcp", "NetworkError"]),
            secret_value: resolve_std_type(checked, &["std", "secret", "SecretValue"]),
            stream_error: resolve_std_type(checked, &["std", "stream", "StreamError"]),
            tcp_stream: resolve_std_type(checked, &["std", "net", "tcp", "TcpStream"]),
            tls_error: resolve_std_type(checked, &["std", "tls", "TlsError"]),
            tls_stream: resolve_std_type(checked, &["std", "tls", "TlsStream"]),
        }
    }
}

pub(crate) fn resolve_std_type(
    checked: &CheckedProject,
    expected_path: &[&str],
) -> Option<etas_types::TypeId> {
    let imported = checked.symbols.iter().find_map(|symbol| {
        let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
            return None;
        };
        if !path
            .iter()
            .map(String::as_str)
            .eq(expected_path.iter().copied())
        {
            return None;
        }
        match checked.types.symbol_types.get(&symbol.id) {
            Some(etas_types::SymbolTypeFact::Type { constructor })
            | Some(etas_types::SymbolTypeFact::NominalType { constructor, .. }) => {
                Some(etas_types::TypeId(constructor.0))
            }
            Some(etas_types::SymbolTypeFact::TypeAlias { target, .. }) => Some(*target),
            _ => None,
        }
    });
    imported.or_else(|| {
        let canonical_name = expected_path.join(".");
        checked.type_store.iter().find_map(|(id, ty)| match ty {
            etas_types::Type::Nominal(nominal) if nominal.name == canonical_name => Some(id),
            etas_types::Type::Enum(enumeration) if enumeration.name == canonical_name => Some(id),
            etas_types::Type::Named(named) if named.name == canonical_name => Some(id),
            _ => None,
        })
    })
}

pub(crate) fn compose_continuation(inner: Continuation, outer: Continuation) -> Continuation {
    match inner {
        Continuation::BlockValue => outer,
        inner => Continuation::Chain {
            inner: Box::new(inner),
            outer: Box::new(outer),
        },
    }
}

fn is_pending_host_boundary_signal(signal: &ControlSignal) -> bool {
    matches!(
        signal,
        ControlSignal::Apply(_)
            | ControlSignal::Checkpoint(_)
            | ControlSignal::Block(_)
            | ControlSignal::Expr(_)
            | ControlSignal::Call(_)
            | ControlSignal::Perform(_)
            | ControlSignal::Memory(_)
            | ControlSignal::Session(_)
            | ControlSignal::Console(_)
            | ControlSignal::Command(_)
            | ControlSignal::Model(_)
            | ControlSignal::Host(_)
    )
}

fn compose_signal_continuation(signal: ControlSignal, continuation: Continuation) -> ControlSignal {
    match signal {
        ControlSignal::Apply(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Apply(pending)
        }
        ControlSignal::Block(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Block(pending)
        }
        ControlSignal::Expr(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Expr(pending)
        }
        ControlSignal::Call(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Call(pending)
        }
        ControlSignal::Memory(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Memory(pending)
        }
        ControlSignal::Session(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Session(pending)
        }
        ControlSignal::Perform(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Perform(pending)
        }
        ControlSignal::Console(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Console(pending)
        }
        ControlSignal::Command(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Command(pending)
        }
        ControlSignal::Model(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Model(pending)
        }
        ControlSignal::Host(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Host(pending)
        }
        signal => signal,
    }
}

fn path_name(action: &ResolvedActionRef) -> String {
    action
        .effect
        .path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

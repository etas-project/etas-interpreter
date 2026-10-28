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
mod checked_type;
mod checkpoint;
mod code;
mod const_expr;
mod continuation;
mod expr;
mod field_access;
mod flow;
mod handler;
mod host_enum;
mod host_type_environment;
mod host_value;
pub(crate) use host_value::{host_to_checked_interp_value, host_to_typed_interp_value};
pub(crate) mod collection_ordering;
mod index_access;
pub(crate) mod limit;
mod loop_control;
pub(crate) mod machine;
mod memory_args;
mod memory_commit;
mod memory_intent;
mod memory_page;
mod memory_selection;
mod memory_store;
mod method;
mod operator_expr;
pub(crate) mod ordered_collection;
mod pattern;
mod perform;
mod pipeline;
pub(crate) mod priority_queue;
mod reflection;
mod run_state;
pub use run_state::EvalRunState;
mod collection_call;
mod safe_point;
mod session_context;
mod session_history;
mod session_value;
mod slice_access;
mod spec_method;
mod std_call;
mod std_value;
mod stmt;
mod stmt_assign;
mod stmt_bind;
mod stmt_branch;
mod stmt_retry;
mod stmt_value;
mod storage;
mod stream;
mod try_expr;
mod variant;
pub(crate) use crate::control::{
    CallTarget, CommandDecode, ConsoleDecode, Continuation, ContinuationInput, ControlSignal,
    Frame, HostBoundaryDecode, HostBoundaryRequest, MemoryDecode, ModelDecode, PendingBlock,
    PendingCall, PendingCheckpoint, PendingCommand, PendingConsole, PendingContinuation,
    PendingExpr, PendingHostBoundary, PendingMemory, PendingModel, PendingPerform, PendingSession,
    SessionDecode, SourceToolBinding, StaticMethodKind,
};
pub(crate) use assign_place::{LocalPlaceComponent, LocalPlaceSegment};
use branch::IfStmtResume;
use call::CallArgsState;
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
        ActiveHandlerArmRecord, ActiveHandlerRecord, CheckpointId, ExecutionProgressSnapshot,
        HandlerScopeId, HandlerSnapshot, HostBoundaryLedger, InterpreterCheckpoint,
        ResourceVersionRecord, ResourceVersionSnapshot, RetryAttemptId, RetryAttemptRecord,
        RetrySnapshot, TraceSnapshot, WorkflowEvent, WorkflowStepId,
    },
    plan::{BraceLiteralShape, InterpreterPlan},
    value::{ArrayValue, InterpValue, MapValue, RecordValue, SliceValue},
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
    ModelContent, ModelMessage, ModelRequest, ModelResponse, ModelRole, SecretOperation,
    SecretRequest, StoreRef, StreamOperation, StreamRequest, TcpConnectOperation,
    TcpConnectRequest, TcpEndpoint, TcpStreamRef, TlsConnectOperation, TlsConnectRequest,
    TraceContext, WriteCondition,
};

pub struct EvalContext<'a> {
    pub(crate) handler_floor: usize,
    pub(crate) revision: Option<&'a std::sync::Arc<crate::api::code::PreparedAgentRevision>>,
    code_definitions: Option<std::sync::Arc<crate::api::code::PreparedCodeDefinitions>>,
    syntax_limits: etas_frontend::code::service::NativeSyntaxLimits,
    code_syntax: std::sync::Arc<etas_frontend::code::service::NativeCodeSyntaxOperations>,
    runtime: &'a mut EvalRunState,
    plan_owner: std::cell::OnceCell<std::sync::Arc<InterpreterPlan>>,
    pub checked: &'a CheckedProject,
    pub view: HirTreeView<'a>,
    pub plan: &'a InterpreterPlan,
    pub known_std_types: KnownStdTypes,
    pub entry_item: HirItemId,
    pub entry_args: &'a [InterpValue],
}

impl std::ops::Deref for EvalContext<'_> {
    type Target = EvalRunState;
    fn deref(&self) -> &Self::Target {
        self.runtime
    }
}

impl std::ops::DerefMut for EvalContext<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.runtime
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KnownStdTypes {
    pub storage_error: Option<etas_types::TypeId>,
    pub browser_session: Option<etas_types::TypeId>,
    pub io_error: Option<etas_types::TypeId>,
    pub memory_conflict: Option<etas_types::TypeId>,
    pub memory_version: Option<etas_types::TypeId>,
    pub memory_cursor: Option<etas_types::TypeId>,
    pub network_error: Option<etas_types::TypeId>,
    pub stream_error: Option<etas_types::TypeId>,
    pub tcp_stream: Option<etas_types::TypeId>,
    pub tls_error: Option<etas_types::TypeId>,
    pub tls_stream: Option<etas_types::TypeId>,
}

pub struct EvalContextInput<'a> {
    pub code_definitions: Option<std::sync::Arc<crate::api::code::PreparedCodeDefinitions>>,
    pub syntax_limits: etas_frontend::code::service::NativeSyntaxLimits,
    pub storage_limits: etas_host::StorageLimits,
    pub event_observer: Option<std::sync::Arc<dyn crate::api::RunEventObserver>>,
    pub execution: etas_host::execution::ExecutionScope,
    pub checked: &'a CheckedProject,
    pub plan: &'a InterpreterPlan,
    pub host_context: HostExecutionContext,
    pub model_policy: ModelExecutionPolicy,
    pub execution_limits: ExecutionLimits,
    pub consumed_steps: u64,
    pub current_session: Option<String>,
    pub entry_item: HirItemId,
    pub entry_args: &'a [InterpValue],
}

pub struct EvalProgramInput<'a> {
    pub code_definitions: Option<std::sync::Arc<crate::api::code::PreparedCodeDefinitions>>,
    pub syntax_limits: etas_frontend::code::service::NativeSyntaxLimits,
    pub checked: &'a CheckedProject,
    pub plan: &'a InterpreterPlan,
    pub entry_item: HirItemId,
    pub entry_args: &'a [InterpValue],
}

struct EvalRunStateInput {
    storage_limits: etas_host::StorageLimits,
    event_observer: Option<std::sync::Arc<dyn crate::api::RunEventObserver>>,
    execution: etas_host::execution::ExecutionScope,
    host_context: HostExecutionContext,
    model_policy: ModelExecutionPolicy,
    execution_limits: ExecutionLimits,
    consumed_steps: u64,
    current_session: Option<String>,
}

impl<'a> EvalContextInput<'a> {
    pub fn into_parts(self) -> (EvalProgramInput<'a>, EvalRunState) {
        let Self {
            code_definitions,
            syntax_limits,
            storage_limits,
            event_observer,
            execution,
            checked,
            plan,
            host_context,
            model_policy,
            execution_limits,
            consumed_steps,
            current_session,
            entry_item,
            entry_args,
        } = self;
        (
            EvalProgramInput {
                code_definitions,
                syntax_limits,
                checked,
                plan,
                entry_item,
                entry_args,
            },
            EvalRunState::from_input(EvalRunStateInput {
                storage_limits,
                event_observer,
                execution,
                host_context,
                model_policy,
                execution_limits,
                consumed_steps,
                current_session,
            }),
        )
    }
}

impl<'a> EvalContext<'a> {
    pub(crate) fn scoped_context<'b>(
        &'b mut self,
        program: &'b crate::value::CheckedExecutionProgram,
        handler_floor: usize,
    ) -> EvalContext<'b> {
        let mut scoped = EvalContext::new(
            EvalProgramInput {
                code_definitions: program.definitions.clone(),
                syntax_limits: self.syntax_limits,
                checked: &program.checked,
                plan: &program.plan,
                entry_item: program.plan.entry.item,
                entry_args: &[],
            },
            &mut *self.runtime,
        );
        scoped.handler_floor = handler_floor;
        scoped.revision = program.revision.as_ref();
        scoped.plan_owner = std::cell::OnceCell::from(program.plan.clone());
        scoped
    }

    pub(crate) fn checked_execution_program(&self) -> crate::value::CheckedExecutionProgram {
        crate::value::CheckedExecutionProgram {
            checked: self.checked.clone(),
            plan: self
                .plan_owner
                .get_or_init(|| std::sync::Arc::new(self.plan.clone()))
                .clone(),
            definitions: self.code_definitions.clone(),
            revision: self.revision.cloned(),
        }
    }
    pub(crate) fn scheduling_decision(&mut self, span: Span) -> safe_point::SafePointDecision {
        match self.execution.signal() {
            Ok(signal) => {
                self.runtime
                    .safe_points
                    .observe(&signal, &self.runtime.host_context.budget, span)
            }
            Err(error) => {
                safe_point::SafePointDecision::Fault(crate::control::ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    error.to_string(),
                ))
            }
        }
    }
    pub(crate) fn cancellation_signal(&self, span: etas_core::Span) -> Option<ControlSignal> {
        match self.execution.signal().and_then(|signal| signal.cause()) {
            Ok(Some(cause)) => Some(ControlSignal::Cancelled(cause)),
            Ok(None) => None,
            Err(error) => Some(ControlSignal::missing_checked_fact(error.to_string(), span)),
        }
    }
    pub fn new(input: EvalProgramInput<'a>, runtime: &'a mut EvalRunState) -> Self {
        let EvalProgramInput {
            code_definitions,
            syntax_limits,
            checked,
            plan,
            entry_item,
            entry_args,
        } = input;
        Self {
            code_syntax: code_definitions.as_ref().map_or_else(
                || {
                    std::sync::Arc::new(
                        etas_frontend::code::service::NativeCodeSyntaxOperations::new(
                            syntax_limits,
                        ),
                    )
                },
                |definitions| definitions.definitions().syntax_operations().clone(),
            ),
            handler_floor: 0,
            revision: None,
            code_definitions,
            syntax_limits,
            runtime,
            plan_owner: std::cell::OnceCell::new(),
            checked,
            view: HirTreeView::new(&checked.hir),
            plan,
            known_std_types: plan.known_std_types,
            entry_item,
            entry_args,
        }
    }

    pub(crate) fn consume_execution_step(
        &mut self,
        span: Span,
    ) -> Result<(), crate::control::ExecutionFault> {
        self.runtime.safe_points.consume(
            self.runtime.execution_limits,
            &self.runtime.host_context.budget,
            span,
        )
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

    pub(crate) fn host_trace_digest_key(
        &self,
    ) -> Result<&etas_host::HostTraceDigestKey, etas_host::HostError> {
        self.host_trace_digest_key.as_ref().map_err(Clone::clone)
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
        self.runtime.next_request_id()
    }
}

impl KnownStdTypes {
    pub(crate) fn from_checked(checked: &CheckedProject) -> Self {
        Self {
            browser_session: resolve_std_type(
                checked,
                &["std", "browser", "protocol", "BrowserSession"],
            ),
            io_error: resolve_std_type(checked, &["std", "io", "IOError"]),
            storage_error: resolve_std_type(checked, &["std", "memory", "StorageError"]),
            memory_conflict: resolve_std_type(checked, &["std", "memory", "MemoryConflict"]),
            memory_version: resolve_std_type(checked, &["std", "memory", "MemoryVersion"]),
            memory_cursor: resolve_std_type(checked, &["std", "memory", "MemoryCursor"]),
            network_error: resolve_std_type(checked, &["std", "net", "tcp", "NetworkError"]),
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
            inner: inner.into(),
            outer: outer.into(),
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

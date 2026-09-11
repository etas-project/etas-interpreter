use etas_core::Span;
use etas_hir::{
    HirArg, HirBlockId, HirElseBranch, HirExprId, HirFieldInit, HirItemId, HirMapEntry,
    HirMatchArm, HirPatId, HirRangeBounds, HirStage, HirTypeId, ResolvedActionRef, ScopeId,
    SymbolId,
};
use etas_types::TypeId;

use crate::api::ExecutionLimits;
use crate::value::{
    HostJsonSupportValue, InterpValue, MemorySelectionKind, MessageRoleValue, ModelResponseValue,
    PromptMessage, ProvenanceValue, RangeBounds,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CheckpointId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RetryAttemptId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HandlerScopeId(pub u32);

#[derive(Clone, Debug)]
pub struct InterpreterCheckpoint {
    pub id: CheckpointId,
    pub label: Option<String>,
    pub compilation: CheckpointCompilationIdentity,
    pub entry_item: HirItemId,
    pub args: Vec<InterpValue>,
    pub machine: MachineSnapshot,
    pub handlers: HandlerSnapshot,
    pub retry_state: RetrySnapshot,
    pub trace: TraceSnapshot,
    pub execution_progress: ExecutionProgressSnapshot,
    pub host_state: CheckpointHostState,
    pub storage: StorageSnapshot,
    pub current_session: Option<String>,
    pub resource_versions: ResourceVersionSnapshot,
    pub completed_host_boundaries: HostBoundaryLedger,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageSnapshot {
    pub identity: etas_host::StorageOperationKey,
    pub operations: std::collections::BTreeMap<u32, etas_host::StorageOperationRef>,
    pub writes: Vec<StorageWriteRecord>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageWriteRecord {
    pub request: u32,
    pub evidence: etas_host::StorageWriteEvidence,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CheckpointHostState {
    pub trace: etas_host::TraceContext,
    pub budget: CheckpointBudgetSnapshot,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CheckpointBudgetSnapshot {
    pub limits: etas_host::Budget,
    pub state: etas_host::ExecutionBudgetSnapshot,
}

impl CheckpointBudgetSnapshot {
    pub fn capture(budget: &etas_host::ExecutionBudget) -> Result<Self, etas_host::HostError> {
        Ok(Self {
            limits: budget.limits().clone(),
            state: budget.snapshot()?,
        })
    }

    pub fn restore(&self) -> Result<etas_host::ExecutionBudget, etas_host::HostError> {
        etas_host::ExecutionBudget::restore(self.limits.clone(), self.state.clone())
    }

    pub fn resume_under(
        &self,
        invocation: &etas_host::ExecutionBudget,
    ) -> Result<etas_host::ExecutionBudget, etas_host::HostError> {
        self.restore()?.resume_under(invocation)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionProgressSnapshot {
    pub consumed_steps: u64,
    pub original_limits: ExecutionLimits,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointCompilationIdentity {
    pub schema_version: String,
    pub compiler_version: String,
    pub project_fingerprint: String,
    pub checked_hir_fingerprint: String,
    pub dependency_metadata_fingerprints: Vec<(String, String)>,
    pub entry_semantic_identity: String,
}

#[derive(Clone, Debug, Default)]
pub struct MachineSnapshot {
    pub(crate) frames: Vec<MachineFrameSnapshot>,
}

#[derive(Clone, Debug)]
pub(crate) enum CallTargetSnapshot {
    FlowItem(HirItemId),
    AgentItem(HirItemId),
    ToolItem(HirItemId),
    SpecImplMethod(SymbolId),
    Lambda {
        expr: etas_hir::HirExprId,
        captured: LocalsSnapshot,
    },
    EnumVariant(SymbolId),
    NominalConstructor(etas_types::TypeId),
    PureIntrinsic {
        intrinsic: etas_std::StdIntrinsicId,
        parameter_types: Vec<etas_types::TypeId>,
        result_type: etas_types::TypeId,
    },
    StdIntrinsic {
        intrinsic: etas_std::StdIntrinsicId,
        dispatch: etas_std::IntrinsicDispatch,
        parameter_types: Vec<etas_types::TypeId>,
        result_type: etas_types::TypeId,
    },
    Specialized {
        target: Box<CallTargetSnapshot>,
        type_bindings: Vec<(String, etas_types::TypeId)>,
    },
    Limited {
        target: Box<CallTargetSnapshot>,
        limits: Vec<crate::eval::limit::RuntimeLimit>,
    },
    Composed(Vec<CallTargetSnapshot>),
}

#[derive(Clone, Debug)]
pub(crate) struct LocalsSnapshot {
    pub(crate) locals: Vec<(SymbolId, ValueSnapshot)>,
    pub(crate) type_bindings: Vec<(String, etas_types::TypeId)>,
}

#[derive(Clone, Debug)]
pub(crate) enum ValueSnapshot {
    MemoryWriteIntent(Box<crate::value::MemoryWriteIntentValue>),
    Unit,
    Bool(bool),
    Number(crate::value::NumericValue),
    String(String),
    Bytes(Vec<u8>),
    Json(HostJsonSupportValue),
    Nominal {
        ty: TypeId,
        value: Box<ValueSnapshot>,
    },
    Trust {
        wrapper: etas_types::TrustWrapper,
        value: Box<ValueSnapshot>,
    },
    Prompt(Vec<PromptMessage>),
    Message(MessageSnapshot),
    Conversation(ConversationSnapshot),
    Provenance(ProvenanceValue),
    ModelResponse(ModelResponseValue),
    Command {
        argv: Vec<String>,
        env: Vec<(String, String)>,
        cwd: Option<etas_host::WorkspacePathRef>,
        stdin: Option<Vec<u8>>,
    },
    CommandResult {
        exit_code: i32,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    Tuple(Vec<ValueSnapshot>),
    Array(Vec<ValueSnapshot>),
    List(Vec<ValueSnapshot>),
    Slice(Vec<ValueSnapshot>),
    Map(Vec<(ValueSnapshot, ValueSnapshot)>),
    Set(Vec<ValueSnapshot>),
    Deque(Vec<ValueSnapshot>),
    Queue(Vec<ValueSnapshot>),
    Stack(Vec<ValueSnapshot>),
    PriorityQueue(Vec<(ValueSnapshot, ValueSnapshot)>),
    OrderedMap(Vec<(ValueSnapshot, ValueSnapshot)>),
    OrderedSet(Vec<ValueSnapshot>),
    Range {
        start: Box<ValueSnapshot>,
        end: Box<ValueSnapshot>,
        bounds: RangeBounds,
    },
    Record(Vec<(String, ValueSnapshot)>),
    Variant {
        name: String,
        fields: Vec<ValueSnapshot>,
    },
    OptionNone,
    OptionSome(Box<ValueSnapshot>),
    Callable(CallTargetSnapshot),
    Handler {
        fact_expr: etas_hir::HirExprId,
        handlers: Vec<ActiveHandlerArmRecord>,
    },
    ResourceHandle {
        name: String,
        stable_id: String,
        ty: TypeId,
    },
    WorkspacePath {
        region: String,
        relative: String,
    },
    MemoryStore {
        region_stable_id: String,
        path: Vec<String>,
        key_type: TypeId,
        value_type: TypeId,
    },
    MemorySelection {
        region_stable_id: String,
        path: Vec<String>,
        key_type: TypeId,
        value_type: TypeId,
        kind: MemorySelectionKind,
        predicate: Option<Box<ValueSnapshot>>,
        limit: Option<u32>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct MessageSnapshot {
    pub(crate) id: String,
    pub(crate) from: Option<String>,
    pub(crate) to: Option<String>,
    pub(crate) role: MessageRoleValue,
    pub(crate) session: Option<String>,
    pub(crate) created_at: String,
    pub(crate) payload: Box<ValueSnapshot>,
    pub(crate) provenance: Option<ProvenanceValue>,
}

#[derive(Clone, Debug)]
pub(crate) struct ConversationSnapshot {
    pub(crate) selected_context: Option<etas_host::session::SessionPublishedContext>,
    pub(crate) session: String,
    pub(crate) history_fence: Option<etas_host::session::SessionHistoryFence>,
    pub(crate) messages: Vec<MessageSnapshot>,
    pub(crate) cursor: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum AggregateKindSnapshot {
    Tuple,
    Array,
    List,
    Set,
}

#[derive(Clone, Debug)]
pub(crate) enum StaticMethodKindSnapshot {
    Prompt,
    Message,
    SessionConfig,
    Conversation,
    Range,
    AdvancedCollection(String),
}

#[derive(Clone, Debug)]
pub(crate) enum LocalPlaceSegmentSnapshot {
    Field(String),
    Index(usize),
    MapKey(Box<ValueSnapshot>),
}

#[derive(Clone, Debug)]
pub(crate) enum LocalPlaceComponentSnapshot {
    Field(String),
    Index { base: HirExprId, index: HirExprId },
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SliceExprEvalSnapshot {
    pub(crate) expr: HirExprId,
    pub(crate) base: HirExprId,
    pub(crate) start: HirExprId,
    pub(crate) end: HirExprId,
    pub(crate) bounds: HirRangeBounds,
    pub(crate) span: Span,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelExecutionPolicySnapshot {
    pub(crate) provider: Option<etas_host::ModelProviderId>,
    pub(crate) provider_capabilities: Option<etas_host::ModelProviderCapabilities>,
    pub(crate) model: etas_host::ModelName,
    pub(crate) model_locked: bool,
    pub(crate) tools: Vec<etas_host::ToolSchema>,
    pub(crate) tool_choice: etas_host::ModelToolChoice,
    pub(crate) policy_ref: Option<etas_host::HostValue>,
    pub(crate) options: etas_host::ModelOptions,
    pub(crate) budget: Option<etas_host::Budget>,
    pub(crate) response_decode: ModelResponseDecodeSnapshot,
    pub(crate) max_tool_rounds: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ModelResponseDecodeSnapshot {
    String,
    ModelResponse,
}

#[derive(Clone, Debug)]
pub(crate) enum ContinuationSnapshot {
    ContinueBlock {
        block: HirBlockId,
        next_stmt_index: usize,
        frame: LocalsSnapshot,
    },
    Bind {
        block: HirBlockId,
        next_stmt_index: usize,
        pat: HirPatId,
        span: Span,
        frame: LocalsSnapshot,
    },
    Assign {
        block: HirBlockId,
        next_stmt_index: usize,
        target: HirExprId,
        span: Span,
        frame: LocalsSnapshot,
    },
    AssignTargetIndex {
        block: HirBlockId,
        next_stmt_index: usize,
        root_symbol: SymbolId,
        segments: Vec<LocalPlaceSegmentSnapshot>,
        components: Vec<LocalPlaceComponentSnapshot>,
        next_component_index: usize,
        new_value: ValueSnapshot,
        span: Span,
        frame: LocalsSnapshot,
    },
    FieldReceiver {
        expr: HirExprId,
        field: String,
        span: Span,
        frame: LocalsSnapshot,
    },
    Unary {
        op: etas_hir::HirUnaryOp,
        span: Span,
    },
    BinaryLeft {
        op: etas_hir::HirBinaryOp,
        rhs: HirExprId,
        span: Span,
        frame: LocalsSnapshot,
    },
    BinaryRight {
        op: etas_hir::HirBinaryOp,
        left: ValueSnapshot,
        span: Span,
    },
    AggregateElement {
        kind: AggregateKindSnapshot,
        exprs: Vec<HirExprId>,
        next_index: usize,
        values: Vec<ValueSnapshot>,
        frame: LocalsSnapshot,
    },
    ListConsHead {
        tail: HirExprId,
        span: Span,
        frame: LocalsSnapshot,
    },
    ListConsTail {
        head: ValueSnapshot,
        span: Span,
    },
    RangeStart {
        end: HirExprId,
        bounds: HirRangeBounds,
        frame: LocalsSnapshot,
    },
    RangeEnd {
        start: ValueSnapshot,
        bounds: HirRangeBounds,
    },
    RecordField {
        nominal_type: Option<TypeId>,
        variant_symbol: Option<SymbolId>,
        fields: Vec<HirFieldInit>,
        next_index: usize,
        values: Vec<(String, ValueSnapshot)>,
        frame: LocalsSnapshot,
    },
    MapKey {
        entries: Vec<HirMapEntry>,
        index: usize,
        values: Vec<(ValueSnapshot, ValueSnapshot)>,
        frame: LocalsSnapshot,
    },
    MapValue {
        entries: Vec<HirMapEntry>,
        index: usize,
        key: ValueSnapshot,
        values: Vec<(ValueSnapshot, ValueSnapshot)>,
        frame: LocalsSnapshot,
    },
    IndexBase {
        expr: HirExprId,
        index: HirExprId,
        span: Span,
        frame: LocalsSnapshot,
    },
    IndexValue {
        expr: HirExprId,
        base: ValueSnapshot,
        span: Span,
    },
    SliceBase {
        eval: SliceExprEvalSnapshot,
        frame: LocalsSnapshot,
    },
    SliceStart {
        eval: SliceExprEvalSnapshot,
        base: ValueSnapshot,
        frame: LocalsSnapshot,
    },
    SliceEnd {
        eval: SliceExprEvalSnapshot,
        base: ValueSnapshot,
        start: ValueSnapshot,
    },
    MethodReceiver {
        expr: HirExprId,
        method: String,
        type_args: Vec<HirTypeId>,
        args: Vec<HirArg>,
        span: Span,
        frame: LocalsSnapshot,
    },
    PromptValueMethodArg {
        messages: Vec<PromptMessage>,
        method: String,
        role: crate::value::PromptRole,
        allow_plain_system_content: bool,
        span: Span,
    },
    LocalMethodArgs {
        expr: HirExprId,
        receiver: ValueSnapshot,
        method: String,
        type_args: Vec<HirTypeId>,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<ValueSnapshot>,
        span: Span,
        frame: LocalsSnapshot,
    },
    StaticMethodArgs {
        expr: HirExprId,
        kind: StaticMethodKindSnapshot,
        method: String,
        type_args: Vec<HirTypeId>,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<ValueSnapshot>,
        span: Span,
        frame: LocalsSnapshot,
    },
    SpecMethodReceiver {
        expr: HirExprId,
        receiver_expr: HirExprId,
        spec_symbol: SymbolId,
        spec_args: Vec<HirTypeId>,
        method: String,
        args: Vec<HirArg>,
        span: Span,
        frame: LocalsSnapshot,
    },
    CalleeEval {
        args: Vec<HirArg>,
        span: Span,
        frame: LocalsSnapshot,
    },
    PipelineStageTarget {
        stages: Vec<HirStage>,
        next_stage_index: usize,
        targets: Vec<CallTargetSnapshot>,
        current_limits: Vec<crate::eval::limit::RuntimeLimit>,
        span: Span,
        frame: LocalsSnapshot,
    },
    CallArgs {
        target: CallTargetSnapshot,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<ValueSnapshot>,
        span: Span,
        frame: LocalsSnapshot,
    },
    VariantArgs {
        variant_symbol: SymbolId,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<ValueSnapshot>,
        span: Span,
        frame: LocalsSnapshot,
    },
    PerformArgs {
        expr: HirExprId,
        action: ResolvedActionRef,
        type_args: Vec<HirTypeId>,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<ValueSnapshot>,
        span: Span,
        frame: LocalsSnapshot,
    },
    MemoryArgs {
        region_stable_id: String,
        path: Vec<String>,
        key_type: TypeId,
        value_type: TypeId,
        result_type: etas_types::TypeId,
        method: String,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<ValueSnapshot>,
        span: Span,
        frame: LocalsSnapshot,
    },
    MemorySelectionLimitArgs {
        region_stable_id: String,
        path: Vec<String>,
        key_type: TypeId,
        value_type: TypeId,
        kind: MemorySelectionKind,
        predicate: Option<ValueSnapshot>,
        limit: Option<u32>,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<ValueSnapshot>,
        span: Span,
        frame: LocalsSnapshot,
    },
    IfExpr {
        then_block: HirBlockId,
        else_branch: Option<HirElseBranch>,
        span: Span,
        frame: LocalsSnapshot,
    },
    IfStmt {
        block: HirBlockId,
        next_stmt_index: usize,
        then_block: HirBlockId,
        else_branch: Option<HirElseBranch>,
        span: Span,
        frame: LocalsSnapshot,
    },
    MatchExpr {
        arms: Vec<HirMatchArm>,
        span: Span,
        frame: LocalsSnapshot,
    },
    MatchStmt {
        block: HirBlockId,
        next_stmt_index: usize,
        arms: Vec<HirMatchArm>,
        span: Span,
        frame: LocalsSnapshot,
    },
    HandleHandler {
        handle_expr: HirExprId,
        body: HirExprId,
        handler: HirExprId,
        span: Span,
        frame: LocalsSnapshot,
    },
    PipelineInput {
        stages: Vec<HirStage>,
        span: Span,
        frame: LocalsSnapshot,
    },
    PipelineTarget {
        input: ValueSnapshot,
        span: Span,
    },
    ComposedCall {
        remaining: Vec<CallTargetSnapshot>,
        span: Span,
    },
    RestoreModelPolicy {
        previous: Box<ModelExecutionPolicySnapshot>,
        inner: Box<ContinuationSnapshot>,
    },
    CallBoundary {
        outer: Box<ContinuationSnapshot>,
    },
    ForLoop {
        pat: HirPatId,
        values: Option<Vec<ValueSnapshot>>,
        next_index: usize,
        body: HirBlockId,
        iterations: usize,
        loop_scope: Vec<SymbolId>,
        span: Span,
        frame: LocalsSnapshot,
    },
    WhileLoop {
        cond: HirExprId,
        body: HirBlockId,
        iteration: u32,
        max_iterations: u32,
        resume_after_body: bool,
        span: Span,
        frame: LocalsSnapshot,
    },
    RetryAttempt {
        retry: RetryAttemptRecord,
        body: HirBlockId,
        attempts: usize,
        next_attempt: usize,
        block: HirBlockId,
        next_stmt_index: usize,
        frame: LocalsSnapshot,
    },
    TryExpr {
        expr: HirExprId,
        span: Span,
    },
    MemoryClearDeleteAll {
        region_stable_id: String,
        path: Vec<String>,
        span: Span,
    },
    MemoryClearDeleteNext {
        region_stable_id: String,
        path: Vec<String>,
        remaining_keys: Vec<ValueSnapshot>,
        next_index: usize,
        span: Span,
    },
    HandlerDispatch {
        outer: Box<ContinuationSnapshot>,
    },
    HandleBoundary {
        scope_id: HandlerScopeId,
        inner: Box<ContinuationSnapshot>,
        handlers: Vec<ActiveHandlerArmRecord>,
        span: Span,
        frame: LocalsSnapshot,
    },
    AgentPromptBody {
        item: HirItemId,
        span: Span,
        model_policy: Option<Box<ModelExecutionPolicySnapshot>>,
    },
    ScopedModelPolicy {
        policy: Box<ModelExecutionPolicySnapshot>,
        inner: Box<ContinuationSnapshot>,
    },
    Chain {
        inner: Box<ContinuationSnapshot>,
        outer: Box<ContinuationSnapshot>,
    },
    Return,
    Resume,
    Finish,
    BlockValue,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelLoopFrameSnapshot {
    pub(crate) pending: PendingModelSnapshot,
    pub(crate) round: usize,
    pub(crate) repair: ModelRepairSnapshot,
    pub(crate) last_tool_error: Option<String>,
    pub(crate) remaining_tool_calls: Vec<etas_host::ModelToolCall>,
    pub(crate) completed_tool_result: bool,
    pub(crate) current_host_tool: Option<HostToolProgressSnapshot>,
    pub(crate) boundary_key: String,
    pub(crate) outer_continuation: ContinuationSnapshot,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingModelSnapshot {
    pub(crate) request: super::ModelRequestSnapshot,
    pub(crate) decode: ModelDecodeSnapshot,
    pub(crate) max_tool_rounds: usize,
    pub(crate) source_tools: Vec<SourceToolBindingSnapshot>,
    pub(crate) span: Span,
    pub(crate) continuation: ContinuationSnapshot,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ModelDecodeSnapshot {
    String,
    ModelResponse,
    Typed(TypeId),
}

#[derive(Clone, Debug)]
pub(crate) struct SourceToolBindingSnapshot {
    pub(crate) name: String,
    pub(crate) qualified_name: Option<String>,
    pub(crate) item: HirItemId,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ModelRepairSnapshot {
    pub(crate) attempts: usize,
    pub(crate) last_kind: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct HostToolProgressSnapshot {
    pub(crate) call: etas_host::ModelToolCall,
    pub(crate) boundary_key: String,
}

#[derive(Clone, Debug)]
pub(crate) struct SourceToolReturnFrameSnapshot {
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) binding: SourceToolBindingSnapshot,
    pub(crate) args: etas_host::HostValue,
    pub(crate) boundary_key: String,
    pub(crate) output_schema: Option<etas_host::HostSchema>,
    pub(crate) model_loop: Box<ModelLoopFrameSnapshot>,
}

#[derive(Clone, Debug)]
pub(crate) enum MachineFrameSnapshot {
    Block {
        continuation: ContinuationSnapshot,
    },
    Expr {
        continuation: ContinuationSnapshot,
    },
    Call {
        continuation: ContinuationSnapshot,
        span: Span,
    },
    Continuation {
        continuation: ContinuationSnapshot,
    },
    Handler {
        continuation: ContinuationSnapshot,
    },
    Retry {
        continuation: ContinuationSnapshot,
    },
    ModelLoop(Box<ModelLoopFrameSnapshot>),
    SourceToolReturn(SourceToolReturnFrameSnapshot),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HandlerSnapshot {
    pub handlers: Vec<ActiveHandlerRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveHandlerRecord {
    pub id: HandlerScopeId,
    pub handled_actions: Vec<String>,
    pub handlers: Vec<ActiveHandlerArmRecord>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveHandlerArmRecord {
    pub effect_segments: Vec<String>,
    pub action: String,
    pub action_symbol: Option<SymbolId>,
    pub type_args: Vec<HirTypeId>,
    pub effect_type_args: Vec<TypeId>,
    pub patterns: Vec<HirPatId>,
    pub body: HirBlockId,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetrySnapshot {
    pub attempts: Vec<RetryAttemptRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryAttemptRecord {
    pub id: RetryAttemptId,
    pub ordinal: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceSnapshot {
    pub events_recorded: usize,
    pub next_message: u32,
    pub next_host_request: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceVersionSnapshot {
    pub versions: Vec<ResourceVersionRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceVersionRecord {
    pub resource: String,
    pub version: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostBoundaryLedger {
    pub completed: Vec<CompletedHostBoundary>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompletedHostBoundary {
    pub occurrence: BoundaryOccurrenceId,
    pub kind: String,
    pub key: String,
    pub result: CompletedHostBoundaryResult,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BoundaryOccurrenceId {
    HostRequest(etas_host::HostRequestId),
    SourceToolCall {
        model_request: etas_host::HostRequestId,
        call_id: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum CompletedHostBoundaryResult {
    Runtime(InterpValue),
    Host(etas_host::HostValue),
}

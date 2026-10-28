use etas_core::Span;
use etas_hir::{
    HirArg, HirBlockId, HirElseBranch, HirExprId, HirItemId, HirMatchArm, HirPatId, HirRangeBounds,
    HirTypeId, ResolvedActionRef, ScopeId, SymbolId,
};
use etas_types::TypeId;

use super::{
    CallTargetSnapshotChildren, CallTargetSnapshotLink, ContinuationSnapshotLink,
    ScopedCallableMetadata, SnapshotBox, SnapshotChildren,
};
use crate::api::ExecutionLimits;
use crate::value::{
    HostJsonSupportValue, InterpValue, MemorySelectionKind, MessageRoleValue, ModelResponseValue,
    PromptValue, ProvenanceValue, RangeBounds,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CheckpointId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RetryAttemptId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HandlerScopeId(pub u32);

#[derive(Clone, Debug)]
pub struct InterpreterCheckpoint {
    /// Session-local immutable code pins. The codec never serializes these owners.
    pub(crate) retained_code: Option<std::sync::Arc<crate::api::code::CheckpointCodePins>>,
    pub id: CheckpointId,
    pub label: Option<String>,
    pub compilation: CheckpointCompilationIdentity,
    pub code_revision: Option<CheckpointCodeRevision>,
    pub entry_item: HirItemId,
    pub args: Vec<InterpValue>,
    pub machine: MachineSnapshot,
    pub(crate) model_policy: ModelExecutionPolicySnapshot,
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

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckpointAgentReference {
    pub revision: CheckpointCodeRevision,
    pub callable: etas_code::CallableId,
    pub frozen_agent: Option<HirItemId>,
}

impl CheckpointAgentReference {
    pub(crate) fn retained_bytes(&self) -> u64 {
        etas_core::memory::MemoryFootprint::of(self).estimated_bytes()
    }

    pub(crate) fn validate_resources(
        &self,
        limits: &etas_host::StorageLimits,
    ) -> Result<(), String> {
        if self.retained_bytes() > limits.max_value_bytes as u64 {
            return Err("checkpoint agent reference exceeds byte budget".into());
        }
        Ok(())
    }
}

/// Pins the semantic revision in addition to the checked program's local IDs.
/// This data is not a compiler lease and cannot prepare executable code.
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
#[serde(deny_unknown_fields)]
pub struct CheckpointCodeRevision {
    pub target: etas_code::DefinitionKey,
    pub revision_key: etas_code::RevisionKey,
    pub revision: etas_code::RevisionInputs,
    pub snapshot_fingerprint: etas_code::CodeFingerprint,
}

impl CheckpointCodeRevision {
    pub(crate) fn for_revision(revision: &crate::api::code::PreparedAgentRevision) -> Self {
        Self {
            target: revision.identity().clone(),
            revision_key: revision.revision().key(),
            revision: revision.revision().inputs().clone(),
            snapshot_fingerprint: revision.snapshot_fingerprint(),
        }
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        if etas_code::RevisionKey::from_inputs(&self.revision) != self.revision_key {
            return Err("checkpoint code revision key differs from its inputs".into());
        }
        if self.target.kind() != etas_code::DefinitionKind::Agent
            || !self.revision.definition.as_method().is_some_and(|method| {
                method.implementation().subject() == &self.target
                    && method.name() == "run"
                    && method.implementation().selected_spec().is_none()
            })
        {
            return Err(
                "checkpoint code revision does not identify an agent default method".into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct MachineSnapshot {
    pub(crate) frames: Vec<MachineFrameSnapshot>,
    pub(crate) program_owners: FrameProgramOwners,
    pub(crate) suspended_programs: Vec<super::SuspendedProgramSnapshot>,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum FrameProgramOwners {
    #[default]
    Root,
    Scoped(Vec<usize>),
}

impl MachineSnapshot {
    pub(crate) fn capture(frames: Vec<MachineFrameSnapshot>) -> Self {
        if !frames
            .iter()
            .any(|frame| matches!(frame, MachineFrameSnapshot::ProgramReturn(_)))
        {
            return Self {
                frames,
                program_owners: FrameProgramOwners::Root,
                suspended_programs: Vec::new(),
            };
        }
        let mut owner = 0;
        let program_owners = frames
            .iter()
            .map(|frame| {
                let current = owner;
                if matches!(frame, MachineFrameSnapshot::ProgramReturn(_)) {
                    owner += 1;
                }
                current
            })
            .collect();
        Self {
            frames,
            program_owners: FrameProgramOwners::Scoped(program_owners),
            suspended_programs: Vec::new(),
        }
    }

    pub(crate) fn program_owner(&self, index: usize) -> usize {
        match &self.program_owners {
            FrameProgramOwners::Root => 0,
            FrameProgramOwners::Scoped(owners) => owners[index],
        }
    }

    pub(crate) fn validate_program_owners(&self) -> Result<(), String> {
        if let FrameProgramOwners::Scoped(owners) = &self.program_owners
            && self.frames.len() != owners.len()
        {
            return Err("checkpoint frame program ownership table has a different length".into());
        }
        let mut owner = 0;
        for (index, frame) in self.frames.iter().enumerate() {
            if self.program_owner(index) != owner {
                return Err(
                    "checkpoint frame program owner does not match its active boundary".into(),
                );
            }
            if matches!(frame, MachineFrameSnapshot::ProgramReturn(_)) {
                owner += 1;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CallTargetSnapshot {
    Scoped {
        metadata: Box<ScopedCallableMetadata>,
        target: CallTargetSnapshotLink,
    },
    FlowItem(HirItemId),
    AgentItem(HirItemId),
    AgentRevision(Box<CheckpointAgentReference>),
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
        origin: HirExprId,
        substitutions: Vec<Vec<(String, etas_types::TypeId)>>,
        intrinsic: etas_std::StdIntrinsicId,
        dispatch: etas_std::IntrinsicDispatch,
        parameter_types: Vec<etas_types::TypeId>,
        result_type: etas_types::TypeId,
    },
    Specialized {
        target: CallTargetSnapshotLink,
        type_bindings: Vec<(String, etas_types::TypeId)>,
        effect_row_bindings: Vec<(String, etas_types::EffectRowRef)>,
    },
    Limited {
        target: CallTargetSnapshotLink,
        limits: Vec<crate::eval::limit::RuntimeLimit>,
    },
    Composed(CallTargetSnapshotChildren),
}

#[derive(Clone, Debug)]
pub(crate) struct LocalsSnapshot {
    pub(crate) id: u64,
    // Owned checkpoint data, never a live Frame backing. Sharing only avoids
    // copying an already captured definition during validation and restoration.
    pub(crate) locals: std::rc::Rc<Vec<(SymbolId, ValueSnapshot)>>,
    pub(crate) type_bindings: Vec<(String, etas_types::TypeId)>,
    pub(crate) effect_row_bindings: Vec<(String, etas_types::EffectRowRef)>,
}

impl PartialEq for LocalsSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.type_bindings == other.type_bindings
            && self.effect_row_bindings == other.effect_row_bindings
            && (std::rc::Rc::ptr_eq(&self.locals, &other.locals) || self.locals == other.locals)
    }
}

impl LocalsSnapshot {
    pub(crate) fn generic_bindings(&self) -> crate::control::GenericBindings {
        crate::control::GenericBindings {
            types: self.type_bindings.iter().cloned().collect(),
            rows: self.effect_row_bindings.iter().cloned().collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ValueSnapshot {
    MemoryWriteIntent(Box<crate::value::MemoryWriteIntentValue>),
    Unit,
    Bool(bool),
    Number(crate::value::NumericValue),
    // Immutable backing is shared with runtime values; mutations use COW.
    String(crate::value::StringValue),
    Bytes(crate::value::BytesValue),
    Json(HostJsonSupportValue),
    Nominal {
        ty: TypeId,
        value: SnapshotBox,
    },
    Trust {
        wrapper: etas_types::TrustWrapper,
        value: SnapshotBox,
    },
    Prompt(PromptValue),
    Schema(TypeId),
    CodeIdentity {
        ty: TypeId,
        identity: crate::value::CodeIdentitySnapshot,
    },
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
    Tuple(SnapshotChildren<ValueSnapshot>),
    Array(SnapshotChildren<ValueSnapshot>),
    List(SnapshotChildren<ValueSnapshot>),
    Slice(SnapshotChildren<ValueSnapshot>),
    Map(SnapshotChildren<(ValueSnapshot, ValueSnapshot)>),
    Set(SnapshotChildren<ValueSnapshot>),
    Deque(SnapshotChildren<ValueSnapshot>),
    Queue(SnapshotChildren<ValueSnapshot>),
    Stack(SnapshotChildren<ValueSnapshot>),
    PriorityQueue(SnapshotChildren<(ValueSnapshot, ValueSnapshot)>),
    OrderedMap(SnapshotChildren<(ValueSnapshot, ValueSnapshot)>),
    OrderedSet(SnapshotChildren<ValueSnapshot>),
    Range {
        start: SnapshotBox,
        end: SnapshotBox,
        bounds: RangeBounds,
    },
    Record(SnapshotChildren<(String, ValueSnapshot)>),
    Variant {
        name: String,
        fields: SnapshotChildren<ValueSnapshot>,
    },
    OptionNone,
    OptionSome(SnapshotBox),
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
        predicate: Option<SnapshotBox>,
        limit: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MessageSnapshot {
    pub(crate) id: String,
    pub(crate) from: Option<String>,
    pub(crate) to: Option<String>,
    pub(crate) role: MessageRoleValue,
    pub(crate) session: Option<String>,
    pub(crate) created_at: String,
    pub(crate) payload: SnapshotBox,
    pub(crate) provenance: Option<ProvenanceValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConversationSnapshot {
    pub(crate) selected_context: Option<crate::value::PublishedContextValue>,
    pub(crate) session: String,
    pub(crate) history_fence: Option<etas_host::session::SessionHistoryFence>,
    pub(crate) messages: Vec<MessageSnapshot>,
    pub(crate) cursor: Option<String>,
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

impl ModelExecutionPolicySnapshot {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self
            .options
            .temperature
            .is_some_and(|value| !value.is_finite())
        {
            return Err("checkpoint model temperature must be finite".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ModelResponseDecodeSnapshot {
    String,
    ModelResponse,
}

#[derive(Clone, Debug)]
pub(crate) enum ContinuationSnapshot {
    ProgramResume {
        id: u32,
    },
    StreamReadReduction {
        call: CallTargetSnapshot,
        args: Vec<ValueSnapshot>,
        limit: usize,
        deadline_unix_ms: Option<u64>,
        body: ValueSnapshot,
        span: Span,
    },
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
        expr: HirExprId,
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
        expr: HirExprId,
        nominal_type: Option<TypeId>,
        variant_symbol: Option<SymbolId>,
        next_index: usize,
        values: Vec<(String, ValueSnapshot)>,
        frame: LocalsSnapshot,
    },
    MapKey {
        expr: HirExprId,
        index: usize,
        values: Vec<(ValueSnapshot, ValueSnapshot)>,
        frame: LocalsSnapshot,
    },
    MapValue {
        expr: HirExprId,
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
        frame: LocalsSnapshot,
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
        frame: LocalsSnapshot,
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
        messages: PromptValue,
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
        call: HirExprId,
        args: std::sync::Arc<[HirArg]>,
        span: Span,
        frame: LocalsSnapshot,
    },
    PipelineStageTarget {
        expr: HirExprId,
        next_stage_index: usize,
        targets: Vec<CallTargetSnapshot>,
        span: Span,
        frame: LocalsSnapshot,
    },
    CallArgs {
        call: HirExprId,
        target: CallTargetSnapshot,
        args: std::sync::Arc<[HirArg]>,
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
        args: std::sync::Arc<[HirArg]>,
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
        expr: HirExprId,
        span: Span,
        frame: LocalsSnapshot,
    },
    PipelineTarget {
        expr: HirExprId,
        input: ValueSnapshot,
        span: Span,
        frame: LocalsSnapshot,
    },
    ComposedCall {
        remaining: Vec<CallTargetSnapshot>,
        site: Option<CallSiteSnapshot>,
        span: Span,
    },
    RestoreModelPolicy {
        previous: Box<ModelExecutionPolicySnapshot>,
        inner: ContinuationSnapshotLink,
    },
    CallBoundary {
        span: Span,
        outer: ContinuationSnapshotLink,
    },
    ForLoop {
        pat: HirPatId,
        source: Option<ValueSnapshot>,
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
        outer: ContinuationSnapshotLink,
    },
    HandleBoundary {
        scope_id: HandlerScopeId,
        inner: ContinuationSnapshotLink,
        handlers: Vec<ActiveHandlerArmRecord>,
        span: Span,
        frame: LocalsSnapshot,
    },
    AgentInference {
        expr: HirExprId,
        span: Span,
        model_policy: Box<ModelExecutionPolicySnapshot>,
    },
    ScopedModelPolicy {
        policy: Box<ModelExecutionPolicySnapshot>,
        inner: ContinuationSnapshotLink,
    },
    Chain {
        inner: ContinuationSnapshotLink,
        outer: ContinuationSnapshotLink,
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
    ProgramReturn(Box<ProgramReturnSnapshot>),
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

impl MachineFrameSnapshot {
    pub(crate) fn continuation_roots(&self) -> [Option<&ContinuationSnapshot>; 2] {
        match self {
            Self::ProgramReturn(frame) => [Some(&frame.outer), None],
            Self::Block { continuation }
            | Self::Expr { continuation }
            | Self::Call { continuation, .. }
            | Self::Continuation { continuation }
            | Self::Handler { continuation }
            | Self::Retry { continuation } => [Some(continuation), None],
            Self::ModelLoop(frame) => [
                Some(&frame.pending.continuation),
                Some(&frame.outer_continuation),
            ],
            Self::SourceToolReturn(frame) => [
                Some(&frame.model_loop.pending.continuation),
                Some(&frame.model_loop.outer_continuation),
            ],
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProgramReturnSnapshot {
    pub call: CallTargetSnapshot,
    pub call_site: CallSiteSnapshot,
    pub handler_floor: usize,
    pub outer: ContinuationSnapshot,
    pub parent_model_policy: ModelExecutionPolicySnapshot,
    pub span: Span,
    pub retry_depth: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct CallSiteSnapshot {
    pub kind: crate::control::CallSiteKind,
    pub frame: LocalsSnapshot,
    pub composition: Option<ComposedInvocationSnapshot>,
}

#[derive(Clone, Debug)]
pub(crate) struct ComposedInvocationSnapshot {
    pub root: CallTargetSnapshotLink,
    pub path: Vec<usize>,
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
    pub arm: etas_hir::HirHandlerArmId,
    pub effect_segments: Vec<String>,
    pub action: String,
    pub action_symbol: Option<SymbolId>,
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

use etas_core::Span;
use etas_hir::{
    HirArg, HirBlockId, HirElseBranch, HirExprId, HirItemId, HirMatchArm, HirPatId, HirStage,
    ResolvedActionRef, SymbolId,
};

use crate::{
    control::Frame,
    intrinsic::dispatch::{CheckedPureIntrinsicCall, CheckedStdIntrinsicCall},
    orchestration::{ActiveHandlerArmRecord, HandlerScopeId, RetryAttemptRecord},
    value::InterpValue,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallTarget {
    FlowItem(HirItemId),
    AgentItem(HirItemId),
    ToolItem(HirItemId),
    SpecImplMethod(SymbolId),
    Lambda {
        expr: HirExprId,
        captured: Frame,
    },
    EnumVariant(SymbolId),
    NominalConstructor(etas_types::TypeId),
    PureIntrinsic(CheckedPureIntrinsicCall),
    StdIntrinsic(CheckedStdIntrinsicCall),
    Specialized {
        target: Box<CallTarget>,
        type_bindings: Vec<(String, etas_types::TypeId)>,
    },
    Limited {
        target: Box<CallTarget>,
        limits: Vec<crate::eval::limit::RuntimeLimit>,
    },
    Composed(Vec<CallTarget>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggregateKind {
    Tuple,
    Array,
    List,
    Set,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StaticMethodKind {
    Prompt,
    Message,
    SessionConfig,
    Conversation,
    Range,
    AdvancedCollection(String),
}

#[derive(Clone, Debug)]
pub enum Continuation {
    ContinueBlock {
        block: HirBlockId,
        next_stmt_index: usize,
        frame: Frame,
    },
    Bind {
        block: HirBlockId,
        next_stmt_index: usize,
        pat: HirPatId,
        span: Span,
        frame: Frame,
    },
    Assign {
        block: HirBlockId,
        next_stmt_index: usize,
        target: HirExprId,
        span: Span,
        frame: Frame,
    },
    AssignTargetIndex {
        block: HirBlockId,
        next_stmt_index: usize,
        root_symbol: SymbolId,
        segments: Vec<crate::eval::LocalPlaceSegment>,
        components: Vec<crate::eval::LocalPlaceComponent>,
        next_component_index: usize,
        new_value: InterpValue,
        span: Span,
        frame: Frame,
    },
    FieldReceiver {
        expr: HirExprId,
        field: String,
        span: Span,
        frame: Frame,
    },
    Unary {
        op: etas_hir::HirUnaryOp,
        span: Span,
    },
    BinaryLeft {
        op: etas_hir::HirBinaryOp,
        rhs: HirExprId,
        span: Span,
        frame: Frame,
    },
    BinaryRight {
        op: etas_hir::HirBinaryOp,
        left: InterpValue,
        span: Span,
    },
    AggregateElement {
        kind: AggregateKind,
        exprs: Vec<HirExprId>,
        next_index: usize,
        values: Vec<InterpValue>,
        frame: Frame,
    },
    ListConsHead {
        tail: HirExprId,
        span: Span,
        frame: Frame,
    },
    ListConsTail {
        head: InterpValue,
        span: Span,
    },
    RangeStart {
        end: HirExprId,
        bounds: etas_hir::HirRangeBounds,
        frame: Frame,
    },
    RangeEnd {
        start: InterpValue,
        bounds: etas_hir::HirRangeBounds,
    },
    RecordField {
        nominal_type: Option<etas_types::TypeId>,
        fields: Vec<etas_hir::HirFieldInit>,
        next_index: usize,
        values: Vec<(String, InterpValue)>,
        frame: Frame,
    },
    MapKey {
        entries: Vec<etas_hir::HirMapEntry>,
        index: usize,
        values: Vec<(InterpValue, InterpValue)>,
        frame: Frame,
    },
    MapValue {
        entries: Vec<etas_hir::HirMapEntry>,
        index: usize,
        key: InterpValue,
        values: Vec<(InterpValue, InterpValue)>,
        frame: Frame,
    },
    IndexBase {
        expr: HirExprId,
        index: HirExprId,
        span: Span,
        frame: Frame,
    },
    IndexValue {
        expr: HirExprId,
        base: InterpValue,
        span: Span,
    },
    SliceBase {
        eval: crate::eval::SliceExprEval,
        frame: Frame,
    },
    SliceStart {
        eval: crate::eval::SliceExprEval,
        base: InterpValue,
        frame: Frame,
    },
    SliceEnd {
        eval: crate::eval::SliceExprEval,
        base: InterpValue,
        start: InterpValue,
    },
    MethodReceiver {
        expr: HirExprId,
        method: String,
        type_args: Vec<etas_hir::HirTypeId>,
        args: Vec<HirArg>,
        span: Span,
        frame: Frame,
    },
    PromptValueMethodArg {
        messages: Vec<crate::value::PromptMessage>,
        method: String,
        role: crate::value::PromptRole,
        allow_plain_system_content: bool,
        span: Span,
    },
    LocalMethodArgs {
        expr: HirExprId,
        receiver: InterpValue,
        method: String,
        type_args: Vec<etas_hir::HirTypeId>,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<InterpValue>,
        span: Span,
        frame: Frame,
    },
    StaticMethodArgs {
        expr: HirExprId,
        kind: StaticMethodKind,
        method: String,
        type_args: Vec<etas_hir::HirTypeId>,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<InterpValue>,
        span: Span,
        frame: Frame,
    },
    SpecMethodReceiver {
        expr: HirExprId,
        receiver_expr: HirExprId,
        spec_symbol: SymbolId,
        spec_args: Vec<etas_hir::HirTypeId>,
        method: String,
        args: Vec<HirArg>,
        span: Span,
        frame: Frame,
    },
    CalleeEval {
        args: Vec<HirArg>,
        span: Span,
        frame: Frame,
    },
    PipelineStageTarget {
        stages: Vec<HirStage>,
        next_stage_index: usize,
        targets: Vec<CallTarget>,
        current_limits: Vec<crate::eval::limit::RuntimeLimit>,
        span: Span,
        frame: Frame,
    },
    CallArgs {
        target: CallTarget,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<InterpValue>,
        span: Span,
        frame: Frame,
    },
    VariantArgs {
        variant_symbol: SymbolId,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<InterpValue>,
        span: Span,
        frame: Frame,
    },
    PerformArgs {
        expr: HirExprId,
        action: ResolvedActionRef,
        type_args: Vec<etas_hir::HirTypeId>,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<InterpValue>,
        span: Span,
        frame: Frame,
    },
    MemoryArgs {
        region_stable_id: String,
        path: Vec<String>,
        key_type: etas_types::TypeId,
        value_type: etas_types::TypeId,
        result_type: etas_types::TypeId,
        method: String,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<InterpValue>,
        span: Span,
        frame: Frame,
    },
    MemorySelectionLimitArgs {
        region_stable_id: String,
        path: Vec<String>,
        key_type: etas_types::TypeId,
        value_type: etas_types::TypeId,
        kind: crate::value::MemorySelectionKind,
        predicate: Option<InterpValue>,
        limit: Option<u32>,
        args: Vec<HirArg>,
        next_arg_index: usize,
        evaluated_args: Vec<InterpValue>,
        span: Span,
        frame: Frame,
    },
    IfExpr {
        then_block: HirBlockId,
        else_branch: Option<HirElseBranch>,
        span: Span,
        frame: Frame,
    },
    IfStmt {
        block: HirBlockId,
        next_stmt_index: usize,
        then_block: HirBlockId,
        else_branch: Option<HirElseBranch>,
        span: Span,
        frame: Frame,
    },
    MatchExpr {
        arms: Vec<HirMatchArm>,
        span: Span,
        frame: Frame,
    },
    MatchStmt {
        block: HirBlockId,
        next_stmt_index: usize,
        arms: Vec<HirMatchArm>,
        span: Span,
        frame: Frame,
    },
    HandleHandler {
        handle_expr: HirExprId,
        body: HirExprId,
        handler: HirExprId,
        span: Span,
        frame: Frame,
    },
    PipelineInput {
        stages: Vec<HirStage>,
        span: Span,
        frame: Frame,
    },
    PipelineTarget {
        input: InterpValue,
        span: Span,
    },
    ComposedCall {
        remaining: Vec<CallTarget>,
        span: Span,
    },
    RestoreModelPolicy {
        previous: Box<crate::api::ModelExecutionPolicy>,
        inner: Box<Continuation>,
    },
    CallBoundary {
        outer: Box<Continuation>,
    },
    ForLoop {
        pat: HirPatId,
        values: Option<Vec<InterpValue>>,
        next_index: usize,
        body: HirBlockId,
        iterations: usize,
        loop_scope: std::collections::HashSet<SymbolId>,
        span: Span,
        frame: Frame,
    },
    WhileLoop {
        cond: HirExprId,
        body: HirBlockId,
        iteration: u32,
        max_iterations: u32,
        resume_after_body: bool,
        span: Span,
        frame: Frame,
    },
    RetryAttempt {
        retry: RetryAttemptRecord,
        body: HirBlockId,
        attempts: usize,
        next_attempt: usize,
        block: HirBlockId,
        next_stmt_index: usize,
        frame: Frame,
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
        remaining_keys: Vec<InterpValue>,
        next_index: usize,
        span: Span,
    },
    HandlerDispatch {
        outer: Box<Continuation>,
    },
    HandleBoundary {
        scope_id: HandlerScopeId,
        inner: Box<Continuation>,
        handlers: Vec<ActiveHandlerArmRecord>,
        span: Span,
        frame: Frame,
    },
    AgentPromptBody {
        item: HirItemId,
        span: Span,
        model_policy: Option<Box<crate::api::ModelExecutionPolicy>>,
    },
    ScopedModelPolicy {
        policy: Box<crate::api::ModelExecutionPolicy>,
        inner: Box<Continuation>,
    },
    Chain {
        inner: Box<Continuation>,
        outer: Box<Continuation>,
    },
    Return,
    Resume,
    Finish,
    BlockValue,
}

impl Continuation {
    pub(crate) fn handler_scope_occurrences(&self, scope_id: HandlerScopeId) -> usize {
        let mut count = 0;
        let mut pending = vec![self];
        while let Some(continuation) = pending.pop() {
            match continuation {
                Self::HandleBoundary {
                    scope_id: boundary_scope,
                    inner,
                    ..
                } => {
                    count += usize::from(*boundary_scope == scope_id);
                    pending.push(inner);
                }
                Self::RestoreModelPolicy { inner, .. } | Self::ScopedModelPolicy { inner, .. } => {
                    pending.push(inner)
                }
                Self::CallBoundary { outer } | Self::HandlerDispatch { outer } => {
                    pending.push(outer);
                }
                Self::Chain { inner, outer } => {
                    pending.push(outer);
                    pending.push(inner);
                }
                _ => {}
            }
        }
        count
    }
}

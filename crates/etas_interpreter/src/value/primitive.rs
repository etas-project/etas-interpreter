use super::{
    aggregate::{ArrayValue, MapValue, SliceValue},
    deque::DequeValue,
    list::ListValue,
    record::RecordValue,
    resource::MemorySelectionKind,
    set::SetValue,
    support::{
        ConversationValue, HostJsonSupportValue, MessageValue, ModelResponseValue, ProvenanceValue,
        RangeValue,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InterpValue {
    Unit,
    Bool(bool),
    Number(super::NumericValue),
    String(super::StringValue),
    Bytes(super::BytesValue),
    Json(HostJsonSupportValue),
    Nominal {
        ty: etas_types::TypeId,
        value: super::SharedValue,
    },
    Trust {
        wrapper: etas_types::TrustWrapper,
        value: super::SharedValue,
    },
    Prompt(super::PromptValue),
    Message(MessageValue),
    Conversation(ConversationValue),
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
    Tuple(super::SharedFields),
    Array(ArrayValue),
    List(ListValue),
    Slice(SliceValue),
    Map(MapValue),
    Set(SetValue),
    Deque(DequeValue),
    Queue(DequeValue),
    Stack(ArrayValue),
    PriorityQueue(MapValue),
    OrderedMap(MapValue),
    OrderedSet(SetValue),
    Range(RangeValue),
    Record(RecordValue),
    Variant {
        name: super::StringValue,
        fields: super::SharedFields,
    },
    OptionNone,
    OptionSome(super::SharedValue),
    Callable(crate::control::CallTarget),
    Handler {
        fact_expr: etas_hir::HirExprId,
        handlers: Vec<crate::orchestration::ActiveHandlerArmRecord>,
    },
    HostHandle(super::HostHandleValue),
    MemoryWriteIntent(Box<super::MemoryWriteIntentValue>),
    ResourceHandle {
        name: String,
        stable_id: String,
        ty: etas_types::TypeId,
    },
    WorkspacePath(etas_host::WorkspacePathRef),
    MemoryStore {
        region_stable_id: String,
        path: Vec<String>,
        key_type: etas_types::TypeId,
        value_type: etas_types::TypeId,
    },
    MemorySelection {
        region_stable_id: String,
        path: Vec<String>,
        key_type: etas_types::TypeId,
        value_type: etas_types::TypeId,
        kind: MemorySelectionKind,
        predicate: Option<Box<InterpValue>>,
        limit: Option<u32>,
    },
}

impl InterpValue {
    pub(crate) fn kind_name(&self) -> &'static str {
        match self {
            Self::Unit => "Unit",
            Self::Bool(_) => "Bool",
            Self::Number(_) => "Number",
            Self::String(_) => "String",
            Self::Bytes(_) => "Bytes",
            Self::Json(_) => "Json",
            Self::Nominal { .. } => "Nominal",
            Self::Trust { .. } => "Trust",
            Self::Prompt(_) => "Prompt",
            Self::Message(_) => "Message",
            Self::Conversation(_) => "Conversation",
            Self::Provenance(_) => "Provenance",
            Self::ModelResponse(_) => "ModelResponse",
            Self::Command { .. } => "Command",
            Self::CommandResult { .. } => "CommandResult",
            Self::Tuple(_) => "Tuple",
            Self::Array(_) => "Array",
            Self::List(_) => "List",
            Self::Slice(_) => "Slice",
            Self::Map(_) => "Map",
            Self::Set(_) => "Set",
            Self::Deque(_) => "Deque",
            Self::Queue(_) => "Queue",
            Self::Stack(_) => "Stack",
            Self::PriorityQueue(_) => "PriorityQueue",
            Self::OrderedMap(_) => "OrderedMap",
            Self::OrderedSet(_) => "OrderedSet",
            Self::Range(_) => "Range",
            Self::Record(_) => "Record",
            Self::Variant { .. } => "Variant",
            Self::OptionNone => "OptionNone",
            Self::OptionSome(_) => "OptionSome",
            Self::Callable(_) => "Callable",
            Self::Handler { .. } => "Handler",
            Self::HostHandle(_) => "HostHandle",
            Self::MemoryWriteIntent(_) => "MemoryWriteIntent",
            Self::ResourceHandle { .. } => "ResourceHandle",
            Self::WorkspacePath(_) => "WorkspacePath",
            Self::MemoryStore { .. } => "MemoryStore",
            Self::MemorySelection { .. } => "MemorySelection",
        }
    }

    pub fn i32(value: i32) -> Self {
        Self::Number(super::NumericValue::I32(value))
    }

    pub fn i64(value: i64) -> Self {
        Self::Number(super::NumericValue::I64(value))
    }

    pub fn u8(value: u8) -> Self {
        Self::Number(super::NumericValue::U8(value))
    }

    pub fn u16(value: u16) -> Self {
        Self::Number(super::NumericValue::U16(value))
    }

    pub fn usize(value: usize) -> Self {
        Self::Number(super::NumericValue::usize(value))
    }

    pub fn as_number(&self) -> Option<super::NumericValue> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }
}

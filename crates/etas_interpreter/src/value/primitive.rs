use super::{
    aggregate::{ArrayValue, ListValue, MapValue, RecordValue, SetValue, SliceValue},
    resource::MemorySelectionKind,
    support::{
        ConversationValue, HostJsonSupportValue, MessageValue, ModelResponseValue, PromptMessage,
        ProvenanceValue, RangeValue,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InterpValue {
    Unit,
    Bool(bool),
    Number(super::NumericValue),
    String(String),
    Bytes(Vec<u8>),
    Json(HostJsonSupportValue),
    Nominal {
        ty: etas_types::TypeId,
        value: Box<InterpValue>,
    },
    Trust {
        wrapper: etas_types::TrustWrapper,
        value: Box<InterpValue>,
    },
    Prompt(Vec<PromptMessage>),
    Message(MessageValue),
    Conversation(ConversationValue),
    Provenance(ProvenanceValue),
    ModelResponse(ModelResponseValue),
    Command {
        argv: Vec<String>,
        env: Vec<(String, String)>,
        cwd: Option<String>,
        stdin: Option<Vec<u8>>,
    },
    CommandResult {
        exit_code: i32,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    Tuple(Vec<InterpValue>),
    Array(ArrayValue),
    List(ListValue),
    Slice(SliceValue),
    Map(MapValue),
    Set(SetValue),
    Deque(ArrayValue),
    Queue(ArrayValue),
    Stack(ArrayValue),
    PriorityQueue(MapValue),
    OrderedMap(MapValue),
    OrderedSet(SetValue),
    Range(RangeValue),
    Record(RecordValue),
    Variant {
        name: String,
        fields: Vec<InterpValue>,
    },
    OptionNone,
    OptionSome(Box<InterpValue>),
    Callable(crate::control::CallTarget),
    Handler {
        fact_expr: etas_hir::HirExprId,
        handlers: Vec<crate::orchestration::ActiveHandlerArmRecord>,
    },
    HostHandle(super::HostHandleValue),
    ResourceHandle {
        name: String,
        stable_id: String,
        ty: etas_types::TypeId,
    },
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

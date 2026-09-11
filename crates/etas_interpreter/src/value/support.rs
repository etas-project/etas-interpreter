use super::primitive::InterpValue;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageValue {
    pub id: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub role: MessageRoleValue,
    pub session: Option<String>,
    pub created_at: String,
    pub payload: Box<InterpValue>,
    pub provenance: Option<ProvenanceValue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionConfigValue {
    pub id: String,
    pub context: Option<Box<InterpValue>>,
    pub retention: Option<Box<InterpValue>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationValue {
    pub selected_context: Option<Box<etas_host::session::SessionPublishedContext>>,
    pub session: String,
    pub history_fence: Option<etas_host::session::SessionHistoryFence>,
    pub messages: Vec<MessageValue>,
    pub cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageRoleValue {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenanceValue {
    pub trace_id: Option<String>,
    pub source: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelResponseValue {
    pub id: u32,
    pub message: ModelMessageValue,
    pub tool_calls: Vec<ModelToolCallValue>,
    pub usage: Option<ModelUsageValue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelMessageValue {
    pub role: ModelRoleValue,
    pub content: Vec<ModelContentValue>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelRoleValue {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelContentValue {
    Text(String),
    Value(HostSupportValue),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelToolCallValue {
    pub id: String,
    pub tool: String,
    pub args: HostSupportValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelUsageValue {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostSupportValue {
    Unit,
    Bool(bool),
    Int(String),
    UInt(String),
    FloatBits(u64),
    String(String),
    Bytes(Vec<u8>),
    List(Vec<HostSupportValue>),
    Map(Vec<(HostSupportValue, HostSupportValue)>),
    Record(Vec<(String, HostSupportValue)>),
    Variant {
        name: String,
        fields: Vec<HostSupportValue>,
    },
    Json(HostJsonSupportValue),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostJsonSupportValue {
    Null,
    Bool(bool),
    NumberBits(u64),
    String(String),
    Array(Vec<HostJsonSupportValue>),
    Object(Vec<(String, HostJsonSupportValue)>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RangeValue {
    pub start: Box<InterpValue>,
    pub end: Box<InterpValue>,
    pub bounds: RangeBounds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeBounds {
    ClosedClosed,
    ClosedOpen,
    OpenOpen,
    OpenClosed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptRole {
    System,
    User,
    Assistant,
    Data,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptMessage {
    pub role: PromptRole,
    pub text: String,
    pub trust: Option<etas_types::TrustWrapper>,
}

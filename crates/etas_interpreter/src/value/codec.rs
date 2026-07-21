use super::{
    resource::MemorySelectionKind,
    support::{MessageRoleValue, ModelRoleValue, PromptRole, RangeBounds},
};
use etas_types::TrustWrapper;

pub(crate) fn trust_wrapper_json(wrapper: TrustWrapper) -> &'static str {
    match wrapper {
        TrustWrapper::Trusted => "trusted",
        TrustWrapper::Untrusted => "untrusted",
        TrustWrapper::Secret => "secret",
        TrustWrapper::Public => "public",
        TrustWrapper::Sanitized => "sanitized",
    }
}

pub(crate) fn trust_wrapper_from_json(wrapper: &str) -> Result<TrustWrapper, String> {
    match wrapper {
        "trusted" => Ok(TrustWrapper::Trusted),
        "untrusted" => Ok(TrustWrapper::Untrusted),
        "secret" => Ok(TrustWrapper::Secret),
        "public" => Ok(TrustWrapper::Public),
        "sanitized" => Ok(TrustWrapper::Sanitized),
        other => Err(format!("unsupported trust wrapper `{other}`")),
    }
}

pub(crate) fn range_bounds_json(bounds: RangeBounds) -> &'static str {
    match bounds {
        RangeBounds::ClosedClosed => "closed_closed",
        RangeBounds::ClosedOpen => "closed_open",
        RangeBounds::OpenOpen => "open_open",
        RangeBounds::OpenClosed => "open_closed",
    }
}

pub(crate) fn model_role_json(role: ModelRoleValue) -> &'static str {
    match role {
        ModelRoleValue::System => "system",
        ModelRoleValue::User => "user",
        ModelRoleValue::Assistant => "assistant",
        ModelRoleValue::Tool => "tool",
    }
}

pub(crate) fn model_role_from_json(role: &str) -> Result<ModelRoleValue, String> {
    match role {
        "system" => Ok(ModelRoleValue::System),
        "user" => Ok(ModelRoleValue::User),
        "assistant" => Ok(ModelRoleValue::Assistant),
        "tool" => Ok(ModelRoleValue::Tool),
        other => Err(format!("unsupported model role `{other}`")),
    }
}

pub(crate) fn message_role_json(role: MessageRoleValue) -> &'static str {
    match role {
        MessageRoleValue::System => "system",
        MessageRoleValue::User => "user",
        MessageRoleValue::Assistant => "assistant",
        MessageRoleValue::Tool => "tool",
    }
}

pub(crate) fn message_role_from_json(role: &str) -> Result<MessageRoleValue, String> {
    match role {
        "system" => Ok(MessageRoleValue::System),
        "user" => Ok(MessageRoleValue::User),
        "assistant" => Ok(MessageRoleValue::Assistant),
        "tool" => Ok(MessageRoleValue::Tool),
        other => Err(format!("unsupported message role `{other}`")),
    }
}

pub(crate) fn range_bounds_from_json(bounds: &str) -> Result<RangeBounds, String> {
    match bounds {
        "closed_closed" => Ok(RangeBounds::ClosedClosed),
        "closed_open" => Ok(RangeBounds::ClosedOpen),
        "open_open" => Ok(RangeBounds::OpenOpen),
        "open_closed" => Ok(RangeBounds::OpenClosed),
        other => Err(format!("unsupported range bounds `{other}`")),
    }
}

pub(crate) fn memory_selection_kind_json(kind: &MemorySelectionKind) -> &'static str {
    match kind {
        MemorySelectionKind::Select => "select",
        MemorySelectionKind::Query => "query",
        MemorySelectionKind::Scan => "scan",
        MemorySelectionKind::RelatedTo => "related_to",
    }
}

pub(crate) fn memory_selection_kind_from_json(kind: &str) -> Result<MemorySelectionKind, String> {
    match kind {
        "select" => Ok(MemorySelectionKind::Select),
        "query" => Ok(MemorySelectionKind::Query),
        "scan" => Ok(MemorySelectionKind::Scan),
        "related_to" => Ok(MemorySelectionKind::RelatedTo),
        other => Err(format!("unsupported memory selection kind `{other}`")),
    }
}

pub(crate) fn prompt_role_json(role: PromptRole) -> &'static str {
    match role {
        PromptRole::System => "system",
        PromptRole::User => "user",
        PromptRole::Assistant => "assistant",
        PromptRole::Data => "data",
    }
}

pub(crate) fn prompt_role_from_json(role: &str) -> Result<PromptRole, String> {
    match role {
        "system" => Ok(PromptRole::System),
        "user" => Ok(PromptRole::User),
        "assistant" => Ok(PromptRole::Assistant),
        "data" => Ok(PromptRole::Data),
        other => Err(format!("unsupported prompt role `{other}`")),
    }
}

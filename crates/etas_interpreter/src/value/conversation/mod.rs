mod budget;
mod context;
mod json;
mod payload;
#[cfg(test)]
mod tests;

use super::ConversationValue;
use crate::orchestration::ConversationSnapshot;
use budget::{MessageView, ViewBudget};
use etas_host::StorageLimits;

pub use context::PublishedContextValue;
pub(crate) use json::validate_json;

pub(crate) fn validate_message(
    value: &super::MessageValue,
    limits: &StorageLimits,
) -> Result<(), String> {
    ViewBudget::new(limits, value.session.as_deref().unwrap_or(""), 1)?.envelope(
        MessageView {
            id: &value.id,
            session: value.session.as_deref(),
            from: value.from.as_deref(),
            to: value.to.as_deref(),
            created_at: &value.created_at,
            payload: value.payload.as_ref(),
            provenance: value
                .provenance
                .as_ref()
                .map(|p| (p.trace_id.as_deref(), p.source.as_deref())),
        },
        0,
    )
}

pub(crate) fn validate_message_snapshot(
    value: &crate::orchestration::MessageSnapshot,
    limits: &StorageLimits,
) -> Result<(), String> {
    ViewBudget::new(limits, value.session.as_deref().unwrap_or(""), 1)?.envelope(
        MessageView {
            id: &value.id,
            session: value.session.as_deref(),
            from: value.from.as_deref(),
            to: value.to.as_deref(),
            created_at: &value.created_at,
            payload: value.payload.as_ref(),
            provenance: value
                .provenance
                .as_ref()
                .map(|p| (p.trace_id.as_deref(), p.source.as_deref())),
        },
        0,
    )
}

pub(crate) fn validate(value: &ConversationValue, limits: &StorageLimits) -> Result<(), String> {
    let mut budget = ViewBudget::new(limits, &value.session, value.messages.len())?;
    budget.metadata(
        value.cursor.as_deref(),
        value.history_fence.as_ref(),
        value.selected_context.as_deref(),
    )?;
    for message in &value.messages {
        budget.message(MessageView {
            id: &message.id,
            session: message.session.as_deref(),
            from: message.from.as_deref(),
            to: message.to.as_deref(),
            created_at: &message.created_at,
            payload: message.payload.as_ref(),
            provenance: message
                .provenance
                .as_ref()
                .map(|p| (p.trace_id.as_deref(), p.source.as_deref())),
        })?;
    }
    Ok(())
}

pub(crate) fn validate_snapshot(
    value: &ConversationSnapshot,
    limits: &StorageLimits,
) -> Result<(), String> {
    let mut budget = ViewBudget::new(limits, &value.session, value.messages.len())?;
    budget.metadata(
        value.cursor.as_deref(),
        value.history_fence.as_ref(),
        value.selected_context.as_deref(),
    )?;
    for message in &value.messages {
        budget.message(MessageView {
            id: &message.id,
            session: message.session.as_deref(),
            from: message.from.as_deref(),
            to: message.to.as_deref(),
            created_at: &message.created_at,
            payload: message.payload.as_ref(),
            provenance: message
                .provenance
                .as_ref()
                .map(|p| (p.trace_id.as_deref(), p.source.as_deref())),
        })?;
    }
    Ok(())
}

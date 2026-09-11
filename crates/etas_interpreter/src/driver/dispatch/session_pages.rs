use super::host_dispatch::HostDispatch;
use crate::{eval::EvalContext, host::HostServices};
use etas_host::{
    HostError, HostErrorCode, HostRequestKind, HostTraceRequest, SessionOperation, SessionRequest,
    SessionResponse, SessionResult,
};

pub(super) async fn execute(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    mut request: SessionRequest,
) -> Result<SessionResponse, HostError> {
    let SessionOperation::Load {
        session,
        context,
        limit: None,
        ..
    } = request.operation.clone()
    else {
        return page(eval, host, request).await;
    };
    let id = request.id;
    let limits = eval.storage_limits.clone();
    let mut messages = Vec::new();
    let mut summary = None;
    let mut selection_fence = None;
    let mut selected_context = None;
    let mut bytes = 0usize;
    let mut cursors = std::collections::BTreeSet::new();
    let mut first = true;
    loop {
        request.budget.check_time()?;
        let response = page(eval, host, request.clone()).await?;
        let SessionResult::History {
            session: actual,
            messages: entries,
            summary: current,
            cursor,
            fence,
            published_context,
        } = response.result?
        else {
            return Err(invalid(
                "session history request returned a different result kind",
            ));
        };
        if actual != session
            || fence.session_id() != session.id
            || entries.iter().any(|message| message.session != session)
        {
            return Err(invalid(
                "session history returned another session's messages",
            ));
        }
        if first {
            bytes = current
                .as_ref()
                .map_or(0, |summary| summary.text.len())
                .checked_add(fence.as_token().len())
                .ok_or_else(exceeded)?;
            summary = current;
            if let Some(context) = &published_context {
                bytes = bytes
                    .checked_add(context.storage_size(&limits)?)
                    .ok_or_else(exceeded)?;
            }
            selected_context = published_context.clone();
            selection_fence = Some(fence.clone());
            first = false;
        } else if summary != current
            || selection_fence.as_ref() != Some(&fence)
            || selected_context != published_context
        {
            return Err(invalid(
                "session summary or selection fence changed during pagination",
            ));
        }
        if messages.len().saturating_add(entries.len()) > limits.max_scan_rows {
            return Err(exceeded());
        }
        for message in &entries {
            bytes = bytes
                .checked_add(message.storage_size(&limits)?)
                .ok_or_else(exceeded)?;
        }
        if bytes > limits.max_result_bytes {
            return Err(exceeded());
        }
        if let Some(cursor) = &cursor {
            if entries.is_empty()
                || cursor.opaque.len() > limits.max_value_bytes
                || !cursors.insert(*blake3::hash(cursor.opaque.as_bytes()).as_bytes())
            {
                return Err(invalid("session history cursor made no progress"));
            }
        }
        messages.extend(entries);
        let Some(cursor) = cursor else {
            return Ok(SessionResponse {
                id,
                result: Ok(SessionResult::History {
                    session,
                    fence,
                    published_context,
                    messages,
                    summary,
                    cursor: None,
                }),
            });
        };
        request.id = eval.next_host_request_id();
        request.operation = SessionOperation::Load {
            session: session.clone(),
            context: context.clone(),
            cursor: Some(cursor),
            limit: None,
        };
    }
}
async fn page(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    request: SessionRequest,
) -> Result<SessionResponse, HostError> {
    HostDispatch::execute(
        eval,
        request.id,
        HostRequestKind::Session,
        request.trace_payload(),
        request.authority.clone(),
        request.trace.clone(),
        |operation| host.session(operation, request.clone()),
    )
    .await
}
fn invalid(message: &str) -> HostError {
    HostError::new(HostErrorCode::InvalidResponse, message)
}
fn exceeded() -> HostError {
    HostError::new(
        HostErrorCode::BudgetExceeded,
        "complete session history exceeds interpreter collection limits",
    )
}

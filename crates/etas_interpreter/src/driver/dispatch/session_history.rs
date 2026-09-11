use super::host_dispatch::HostDispatch;
use crate::{
    control::{ControlSignal, PendingHostBoundary},
    eval::EvalContext,
    host::HostServices,
};
use etas_host::{
    HostRequestKind, HostTraceRequest, HostValue, PolicySubject, SessionOperation, SessionRequest,
};

pub(super) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    boundary: PendingHostBoundary,
    request: SessionRequest,
) -> ControlSignal {
    let SessionOperation::Load {
        session,
        limit: Some(limit),
        ..
    } = &request.operation
    else {
        return ControlSignal::missing_checked_fact(
            "history_page requires a bounded Load request",
            boundary.span,
        );
    };
    let response = HostDispatch::execute(
        eval,
        request.id,
        HostRequestKind::Session,
        request.trace_payload(),
        request.authority.clone(),
        request.trace.clone(),
        |operation| host.session(operation, request.clone()),
    )
    .await;
    if let Some(signal) = eval.cancellation_signal(boundary.span) {
        return signal;
    }
    match response
        .and_then(|response| response.result)
        .and_then(|result| {
            etas_host::session::session_history_page_value(
                &result,
                session,
                *limit,
                &eval.storage_limits.clone(),
            )
        }) {
        Ok(value) => eval.resume_session_history_page(boundary, value),
        Err(error) => {
            eval.storage_error_with_continuation(error, boundary.span, boundary.continuation)
        }
    }
}

pub(super) fn policy_subject(request: &SessionRequest) -> PolicySubject {
    let mut attributes = vec![
        (
            "qualified_action".into(),
            HostValue::String("Memory.read".into()),
        ),
        ("operation".into(), HostValue::String("history_page".into())),
    ];
    if let SessionOperation::Load { session, .. } = &request.operation {
        attributes.push(("session".into(), HostValue::String(session.id.clone())));
        attributes.push(("resource".into(), HostValue::String(session.id.clone())));
    }
    PolicySubject {
        kind: "session".into(),
        attributes,
    }
}

use crate::{
    control::{ControlSignal, PendingSession},
    eval::EvalContext,
    host::HostServices,
};

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    session: PendingSession,
) -> ControlSignal {
    if let Some(value) = eval.replayed_session_result(&session) {
        return eval.replay_session_signal(session, value);
    }
    if let Err(error) = session.request.budget.check_time() {
        return eval.session_host_error_signal(session, error);
    }
    let key = eval.session_boundary_key(&session);
    let request_id = session.request.id;
    let response = if matches!(
        session.request.operation,
        etas_host::SessionOperation::Append { .. } | etas_host::SessionOperation::Resolve { .. }
    ) {
        super::session_write::dispatch(eval, host, session.request.clone()).await
    } else {
        super::session_pages::execute(eval, host, session.request.clone()).await
    };
    if let Some(signal) = eval.cancellation_signal(session.span) {
        return signal;
    }
    match response {
        Ok(response) => match response.result {
            Ok(result) => {
                match eval.session_boundary_result_value(&session, &result) {
                    Ok(Some(value)) => {
                        eval.record_session_result_event(&result, Some(&value));
                        eval.record_completed_host_boundary(
                            crate::orchestration::BoundaryOccurrenceId::HostRequest(request_id),
                            "session",
                            key,
                            value,
                        );
                    }
                    Ok(None) => eval.record_session_result_event(&result, None),
                    Err(error) => {
                        return ControlSignal::runtime_fault(error, session.span);
                    }
                }
                eval.session_result_signal(session, result)
            }
            Err(error) => eval.session_host_error_signal(session, error),
        },
        Err(error) => eval.session_host_error_signal(session, error),
    }
}

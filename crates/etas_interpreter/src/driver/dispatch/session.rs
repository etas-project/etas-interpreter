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
    let key = eval.session_boundary_key(&session);
    let request_id = session.request.id;
    eval.record_host_request_sent(request_id);
    match host.session(session.request.clone()).await {
        Ok(response) => {
            eval.record_host_response_received(response.id);
            match response.result {
                Ok(result) => {
                    eval.record_session_result_event(&result);
                    match eval.session_boundary_result_value(&session, &result) {
                        Ok(Some(value)) => {
                            eval.record_completed_host_boundary("session", key, value);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            return ControlSignal::runtime_fault(error, session.span);
                        }
                    }
                    eval.session_result_signal(session, result)
                }
                Err(error) => eval.session_host_error_signal(session, error),
            }
        }
        Err(error) => {
            eval.record_host_response_received(request_id);
            eval.session_host_error_signal(session, error)
        }
    }
}

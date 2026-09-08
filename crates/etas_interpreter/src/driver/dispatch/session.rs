use crate::{
    control::{ControlSignal, PendingSession},
    eval::EvalContext,
    host::HostServices,
};

use etas_host::{HostRequestKind, HostTraceRequest};

use super::host_dispatch::HostDispatch;

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
    match HostDispatch::execute(
        eval,
        request_id,
        HostRequestKind::Session,
        session.request.trace_payload(),
        session.request.authority.clone(),
        session.request.trace.clone(),
        host.session(session.request.clone()),
    )
    .await
    {
        Ok(response) => match response.result {
            Ok(result) => {
                eval.record_session_result_event(&result);
                match eval.session_boundary_result_value(&session, &result) {
                    Ok(Some(value)) => {
                        eval.record_completed_host_boundary(
                            crate::orchestration::BoundaryOccurrenceId::HostRequest(request_id),
                            "session",
                            key,
                            value,
                        );
                    }
                    Ok(None) => {}
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

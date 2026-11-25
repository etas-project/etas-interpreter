use crate::{
    control::{ControlSignal, PendingSession, SessionDecode},
    eval::EvalContext,
    host::HostServices,
};

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    _host: &dyn HostServices,
    session: PendingSession,
) -> ControlSignal {
    let key = match eval.session_boundary_key(&session) {
        Ok(key) => key,
        Err(fault) => return ControlSignal::Fault(Box::new(fault)),
    };
    match eval.replayed_session_result(&session, &key) {
        Ok(Some(value)) => return eval.replay_session_signal(session, value),
        Ok(None) => (),
        Err(fault) => return ControlSignal::Fault(Box::new(fault)),
    }
    let checks = if matches!(session.decode, SessionDecode::SourceValue { .. }) {
        vec![(
            eval.boundary_policy_ref(),
            policy_subject(&session.request.operation),
        )]
    } else {
        Vec::new()
    };
    let budget = session.request.budget.clone();
    super::authorization::prepare(
        eval,
        crate::control::NativeBoundaryTarget::Session {
            pending: session,
            key,
        },
        checks,
        budget,
    )
}

pub(in crate::driver) async fn dispatch_authorized(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    mut session: PendingSession,
    key: etas_host::replay::ReplayFingerprint,
) -> ControlSignal {
    session.request.authority = eval.host_authority();
    if let Err(error) = session.request.budget.check_time() {
        return eval.session_host_error_signal(session, error);
    }
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
                        if let Err(fault) = eval.record_completed_host_boundary(
                            crate::orchestration::BoundaryOccurrenceId::HostRequest(request_id),
                            "session",
                            key,
                            value.clone(),
                            session.span,
                        ) {
                            return ControlSignal::Fault(Box::new(fault));
                        }
                        return eval.replay_session_signal(session, value);
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

fn policy_subject(operation: &etas_host::SessionOperation) -> etas_host::PolicySubject {
    use etas_host::{HostValue, SessionOperation};
    let (action, operation, session) = match operation {
        SessionOperation::Lease(lease) => (
            "Memory.write",
            lease.qualified_action(),
            &lease.session().id,
        ),
        SessionOperation::Resolve { config } => ("Memory.write", "resolve", &config.id),
        SessionOperation::Append { message } => ("Memory.write", "append", &message.session.id),
        SessionOperation::Load { session, .. } => ("Memory.read", "history_page", &session.id),
    };
    etas_host::PolicySubject {
        kind: "session".into(),
        attributes: vec![
            ("qualified_action".into(), HostValue::String(action.into())),
            ("operation".into(), HostValue::String(operation.into())),
            ("session".into(), HostValue::String(session.clone())),
            ("resource".into(), HostValue::String(session.clone())),
        ],
    }
}

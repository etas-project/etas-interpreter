use super::host_dispatch::HostDispatch;
use crate::{
    eval::EvalContext,
    host::HostServices,
    orchestration::{StorageWriteRecord, WorkflowEvent},
};
use etas_host::session::{
    SessionWriteOperation, SessionWriteReceipt, SessionWriteRequest, SessionWriteResult,
};
use etas_host::{
    HostError, HostErrorCode, HostRequestKind, HostTraceRequest, SessionOperation, SessionRequest,
    SessionResponse, SessionResult, WriteOutcome,
};

pub(super) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    request: SessionRequest,
) -> Result<SessionResponse, HostError> {
    let key = eval
        .storage_identity
        .clone()?
        .derive(u64::from(request.id.0))?;
    let limits = eval.storage_limits.clone();
    let (operation, expected) = match &request.operation {
        SessionOperation::Append { message } => (
            SessionWriteOperation::Append {
                key: key.clone(),
                message: Box::new(message.clone()),
            },
            etas_host::session::append_operation_ref(message, key, &limits)?,
        ),
        SessionOperation::Resolve { config } => (
            SessionWriteOperation::Resolve {
                key: key.clone(),
                config: config.clone(),
            },
            etas_host::session::resolve_operation_ref(config, key, &limits)?,
        ),
        _ => {
            return Err(invalid(
                "non-mutation request entered session write dispatch",
            ));
        }
    };
    eval.bind_storage_operation(request.id, &expected)?;
    let write = SessionWriteRequest {
        id: request.id,
        operation,
        authority: request.authority.clone(),
        trace: request.trace.clone(),
        budget: request.budget.clone(),
    };
    let response = HostDispatch::execute(
        eval,
        request.id,
        HostRequestKind::Session,
        request.trace_payload(),
        request.authority,
        request.trace,
        |operation| async {
            let response = host.session_write(operation, write).await?;
            if let Ok(result) = &response.result {
                let reference = match result {
                    SessionWriteResult::Outcome(WriteOutcome::Committed(receipt)) => {
                        committed_result(&request.operation, receipt)?;
                        receipt.operation()
                    }
                    SessionWriteResult::Outcome(
                        WriteOutcome::NotCommitted { operation, .. }
                        | WriteOutcome::Unknown { operation, .. },
                    ) => operation,
                    _ => return Err(invalid("session mutation returned a reconciliation result")),
                };
                if reference != &expected {
                    return Err(invalid("session receipt belongs to a different operation"));
                }
            }
            Ok(response)
        },
    )
    .await;
    let result = match response.and_then(|response| response.result) {
        Ok(SessionWriteResult::Outcome(WriteOutcome::Committed(receipt))) => {
            committed_result(&request.operation, &receipt)
        }
        Ok(SessionWriteResult::Outcome(WriteOutcome::NotCommitted { reason, .. })) => Err(reason),
        Ok(SessionWriteResult::Outcome(WriteOutcome::Unknown { error, .. })) => Err(unknown(error)),
        Err(error) => {
            if !eval
                .storage_writes
                .iter()
                .any(|write| write.request == request.id.0)
            {
                let evidence = etas_host::StorageWriteEvidence {
                    operation: expected,
                    status: etas_host::CommitStatus::Unknown,
                };
                eval.storage_writes.push(StorageWriteRecord {
                    request: request.id.0,
                    evidence: evidence.clone(),
                });
                eval.events.push(WorkflowEvent::StorageWrite {
                    request: request.id,
                    evidence,
                });
                Err(unknown(error))
            } else {
                Err(error)
            }
        }
        Ok(_) => Err(invalid("session mutation returned a reconciliation result")),
    };
    Ok(SessionResponse {
        id: request.id,
        result,
    })
}
fn committed_result(
    operation: &SessionOperation,
    receipt: &SessionWriteReceipt,
) -> Result<SessionResult, HostError> {
    match (operation, receipt) {
        (SessionOperation::Append { message }, SessionWriteReceipt::Append(receipt)) => {
            if receipt.message_id.is_empty()
                || !receipt.version.belongs_to(&message.session.id)
                || (!receipt.deduplicated && receipt.message_id != message.id)
            {
                return Err(invalid(
                    "session append receipt has an inconsistent message or session identity",
                ));
            }
            let mut message = message.clone();
            message.id = receipt.message_id.clone();
            Ok(SessionResult::Appended {
                message,
                deduplicated: receipt.deduplicated,
            })
        }
        (SessionOperation::Resolve { config }, SessionWriteReceipt::Resolve(receipt)) => {
            if receipt.session.id != config.id || !receipt.generation.belongs_to(&config.id) {
                return Err(invalid(
                    "session resolve receipt has an inconsistent session or generation",
                ));
            }
            Ok(SessionResult::Resolved {
                session: receipt.session.clone(),
                created: receipt.created,
            })
        }
        _ => Err(invalid(
            "session receipt kind does not match the requested mutation",
        )),
    }
}
fn invalid(message: &str) -> HostError {
    HostError::new(HostErrorCode::InvalidResponse, message)
}
pub(super) fn unknown(error: HostError) -> HostError {
    HostError::new(
        error.code,
        format!(
            "session mutation commit outcome is unknown; automatic retry is forbidden: {}",
            error.message
        ),
    )
}

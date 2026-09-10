use super::host_dispatch::HostDispatch;
use crate::{
    control::{ControlSignal, PendingHostBoundary},
    eval::EvalContext,
    host::HostServices,
};
use etas_host::session::{
    SessionContextEvidence, SessionContextReceipt, SessionWriteOperation, SessionWriteRequest,
    SessionWriteResult,
};
use etas_host::{
    HostError, HostErrorCode, HostRequestKind, HostTraceFieldSensitivity, HostTracePayload,
    HostTraceRequest, HostValue, PolicySubject, ReceiptLookup, StorageOperationRef, WriteOutcome,
};
use std::cell::Cell;

pub(super) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    boundary: PendingHostBoundary,
    request: SessionWriteRequest,
) -> ControlSignal {
    let (expected, query) = match &request.operation {
        SessionWriteOperation::PublishContext(publication) => {
            (publication.operation.clone(), false)
        }
        SessionWriteOperation::ReconcileContext { operation, .. } => (operation.clone(), true),
        _ => {
            return ControlSignal::missing_checked_fact(
                "unexpected session context operation",
                boundary.span,
            );
        }
    };
    let payload = match &request.operation {
        SessionWriteOperation::PublishContext(publication) => publication.trace_payload(),
        SessionWriteOperation::ReconcileContext { session, operation } => {
            HostTracePayload::new("session", "Session.reconcile_context")
                .with_field(
                    "session",
                    HostValue::String(session.id.clone()),
                    HostTraceFieldSensitivity::Sensitive,
                )
                .with_field(
                    "operation",
                    HostValue::Record(vec![
                        (
                            "key".into(),
                            HostValue::String(operation.key.as_str().into()),
                        ),
                        (
                            "fingerprint".into(),
                            HostValue::String(operation.request_fingerprint.clone()),
                        ),
                    ]),
                    HostTraceFieldSensitivity::Sensitive,
                )
        }
        _ => {
            return ControlSignal::missing_checked_fact(
                "unexpected session context trace",
                boundary.span,
            );
        }
    };
    if let Err(error) = eval.bind_storage_operation(request.id, &expected) {
        return eval.storage_error_with_continuation(error, boundary.span, boundary.continuation);
    }
    let dispatched = Cell::new(false);
    let response = HostDispatch::execute(
        eval,
        request.id,
        HostRequestKind::Session,
        payload,
        request.authority.clone(),
        request.trace.clone(),
        |operation| async {
            dispatched.set(true);
            let response = host.session_write(operation, request.clone()).await?;
            if let Ok(result) = &response.result {
                validate(&request, &expected, result)?;
            }
            Ok(response)
        },
    )
    .await;
    if let Some(signal) = eval.cancellation_signal(boundary.span) {
        return signal;
    }
    match response.and_then(|response| response.result) {
        Ok(result) => eval.resume_session_context_result(boundary, result),
        Err(error) if query || !dispatched.get() => {
            eval.storage_error_with_continuation(error, boundary.span, boundary.continuation)
        }
        Err(error) => {
            if !eval
                .storage_writes
                .iter()
                .any(|record| record.request == request.id.0)
            {
                let evidence = etas_host::StorageWriteEvidence {
                    operation: expected.clone(),
                    status: etas_host::CommitStatus::Unknown,
                };
                eval.storage_writes
                    .push(crate::orchestration::StorageWriteRecord {
                        request: request.id.0,
                        evidence: evidence.clone(),
                    });
                eval.events
                    .push(crate::orchestration::WorkflowEvent::StorageWrite {
                        request: request.id,
                        evidence,
                    });
            }
            eval.resume_session_context_result(
                boundary,
                SessionWriteResult::Context(WriteOutcome::Unknown {
                    operation: expected,
                    error,
                }),
            )
        }
    }
}

fn validate(
    request: &SessionWriteRequest,
    expected: &StorageOperationRef,
    result: &SessionWriteResult,
) -> Result<(), HostError> {
    let operation = match (&request.operation, result) {
        (
            SessionWriteOperation::PublishContext(publication),
            SessionWriteResult::Context(outcome),
        ) => match outcome {
            WriteOutcome::Committed(receipt) => {
                validate_receipt(&publication.session, receipt)?;
                &receipt.operation
            }
            WriteOutcome::NotCommitted { operation, .. }
            | WriteOutcome::Unknown { operation, .. } => operation,
        },
        (
            SessionWriteOperation::ReconcileContext { session, .. },
            SessionWriteResult::ContextReceipt(lookup),
        ) => match lookup {
            ReceiptLookup::Found(SessionContextEvidence::Committed(receipt)) => {
                validate_receipt(session, receipt)?;
                &receipt.operation
            }
            ReceiptLookup::Found(SessionContextEvidence::NotCommitted { operation, .. }) => {
                operation
            }
            ReceiptLookup::Unresolved | ReceiptLookup::Expired => return Ok(()),
        },
        _ => return Err(invalid()),
    };
    if operation != expected {
        return Err(invalid());
    }
    Ok(())
}
fn validate_receipt(
    session: &etas_host::SessionRef,
    receipt: &SessionContextReceipt,
) -> Result<(), HostError> {
    if &receipt.session != session
        || !receipt.generation.belongs_to(&session.id)
        || receipt.context_version == 0
        || receipt.context_version > i64::MAX as u64
    {
        return Err(invalid());
    }
    Ok(())
}
pub(super) fn policy_subject(request: &SessionWriteRequest) -> Result<PolicySubject, HostError> {
    let (name, action, session) = match &request.operation {
        SessionWriteOperation::PublishContext(publication) => {
            ("publish_context", "Memory.write", &publication.session)
        }
        SessionWriteOperation::ReconcileContext { session, .. } => {
            ("reconcile_context", "Memory.read", session)
        }
        _ => return Err(invalid()),
    };
    let mut attributes = vec![
        ("qualified_action".into(), HostValue::String(action.into())),
        ("operation".into(), HostValue::String(name.into())),
    ];
    attributes.push(("resource".into(), HostValue::String(session.id.clone())));
    Ok(PolicySubject {
        kind: "session".into(),
        attributes,
    })
}
fn invalid() -> HostError {
    HostError::new(
        HostErrorCode::InvalidResponse,
        "context response does not match checked operation, session or result kind",
    )
}

use super::host_dispatch::HostDispatch;
use crate::{
    control::{ControlSignal, PendingHostBoundary},
    eval::EvalContext,
    host::HostServices,
};
use etas_host::memory::{MemoryWriteOperation, MemoryWriteRequest, MemoryWriteResult};
use etas_host::{
    ConfirmedOutcome, HostError, HostErrorCode, HostRequestKind, HostTraceRequest, HostValue,
    PolicySubject, ReceiptLookup, StorageOperationRef, WriteOutcome,
};
use std::cell::Cell;

pub(super) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    boundary: PendingHostBoundary,
    request: MemoryWriteRequest,
) -> ControlSignal {
    let expected = match &request.operation {
        MemoryWriteOperation::Mutate { key, mutation } => {
            mutation.operation_ref(&request.store, key.clone(), &eval.storage_limits.clone())
        }
        MemoryWriteOperation::Reconcile { operation } => {
            operation.validate().map(|()| operation.clone())
        }
    };
    let expected = match expected {
        Ok(expected) => expected,
        Err(error) => {
            return eval.storage_error_with_continuation(
                error,
                boundary.span,
                boundary.continuation,
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
        HostRequestKind::Memory,
        request.trace_payload(),
        request.authority.clone(),
        request.trace.clone(),
        |operation| async {
            dispatched.set(true);
            let response = host.memory_write(operation, request.clone()).await?;
            if let Ok(result) = &response.result {
                validate_result(&request, &expected, result)?;
            }
            Ok(response)
        },
    )
    .await;
    if let Some(signal) = eval.cancellation_signal(boundary.span) {
        return signal;
    }
    match response.and_then(|response| response.result) {
        Ok(result) => eval.resume_storage_result(boundary, result),
        Err(error)
            if !dispatched.get()
                || matches!(request.operation, MemoryWriteOperation::Reconcile { .. }) =>
        {
            eval.storage_error_with_continuation(error, boundary.span, boundary.continuation)
        }
        Err(error) => {
            // A lost/invalid response is not proof of non-commit. Never enter retry_or_report.
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
            eval.resume_storage_result(
                boundary,
                MemoryWriteResult::Outcome(WriteOutcome::Unknown {
                    operation: expected,
                    error,
                }),
            )
        }
    }
}

fn validate_result(
    request: &MemoryWriteRequest,
    expected: &StorageOperationRef,
    result: &MemoryWriteResult,
) -> Result<(), HostError> {
    let operation = match (&request.operation, result) {
        (MemoryWriteOperation::Mutate { mutation, .. }, MemoryWriteResult::Outcome(outcome)) => {
            match outcome {
                WriteOutcome::Committed(receipt) => {
                    let kind = match mutation {
                        etas_host::memory::MemoryMutation::Put { .. } => {
                            etas_host::memory::MemoryMutationKind::Put
                        }
                        _ => etas_host::memory::MemoryMutationKind::Delete,
                    };
                    if receipt.target != mutation.target(&request.store)
                        || receipt.change.kind() != kind
                    {
                        return Err(invalid());
                    }
                    &receipt.operation
                }
                WriteOutcome::NotCommitted { operation, .. }
                | WriteOutcome::Unknown { operation, .. } => operation,
            }
        }
        (MemoryWriteOperation::Reconcile { .. }, MemoryWriteResult::Receipt(lookup)) => {
            match lookup {
                ReceiptLookup::Found(ConfirmedOutcome::Committed(receipt)) => {
                    if receipt.target.store != request.store {
                        return Err(invalid());
                    }
                    &receipt.operation
                }
                ReceiptLookup::Found(ConfirmedOutcome::NotCommitted { operation, .. }) => operation,
                ReceiptLookup::Unresolved | ReceiptLookup::Expired => return Ok(()),
            }
        }
        _ => return Err(invalid()),
    };
    if operation != expected {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn policy_subject(request: &MemoryWriteRequest) -> PolicySubject {
    let (operation, action) = match request.operation {
        MemoryWriteOperation::Mutate { .. } => ("commit", "write"),
        MemoryWriteOperation::Reconcile { .. } => ("reconcile", "read"),
    };
    PolicySubject {
        kind: "memory".into(),
        attributes: vec![
            ("action_kind".into(), HostValue::String("memory".into())),
            (
                "qualified_action".into(),
                HostValue::String(format!("Memory.{action}")),
            ),
            ("operation".into(), HostValue::String(operation.into())),
            (
                "region".into(),
                HostValue::String(request.store.region.stable_id.clone()),
            ),
            (
                "path".into(),
                HostValue::String(request.store.path.join(".")),
            ),
            (
                "resource".into(),
                HostValue::String(format!(
                    "{}:{}",
                    request.store.region.stable_id,
                    request.store.path.join(".")
                )),
            ),
        ],
    }
}
fn invalid() -> HostError {
    HostError::new(
        HostErrorCode::InvalidResponse,
        "memory response does not match the checked operation, target or result kind",
    )
}
